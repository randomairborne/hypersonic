#![warn(clippy::all, clippy::pedantic, clippy::nursery)]

mod load_tracks;

use std::{error::Error, fmt::Display, pin::pin, str::FromStr, sync::Arc, time::Duration};

use rand::seq::SliceRandom;
use songbird::{input::Input, shards::TwilightMap, Call, Songbird};
use tokio::sync::Mutex;
use tokio_util::task::TaskTracker;
use twilight_gateway::{CloseFrame, Event, EventTypeFlags, Intents, Shard, ShardId, StreamExt};
use twilight_http::Client as HttpClient;
use twilight_model::id::{
    marker::{ChannelMarker, GuildMarker},
    Id,
};

#[macro_use]
extern crate tracing;

#[derive(Debug, Clone)]
struct SongMetadata {
    title: Box<str>,
    artist: Option<Box<str>>,
    album: Option<Box<str>>,
}

impl Display for SongMetadata {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (&self.artist, &self.album) {
            (Some(artist), Some(album)) => write!(f, "{} by {artist} ({album})", self.title),
            (Some(artist), None) => write!(f, "{} by {artist}", self.title),
            (None, Some(album)) => write!(f, "{} from {album}", self.title),
            (None, None) => f.write_str(&self.title),
        }
    }
}

#[derive(Debug, Clone)]
struct Song {
    data: Box<[u8]>,
    meta: SongMetadata,
}

#[derive(Debug, Clone)]
struct State {
    http: Arc<HttpClient>,
    songbird: Arc<Songbird>,
    vc: Id<ChannelMarker>,
    guild: Id<GuildMarker>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync + 'static>> {
    tracing_subscriber::fmt::init();

    let token = get_var("DISCORD_TOKEN");
    let vc: Id<ChannelMarker> = parse_var("DISCORD_VC");
    let guild: Id<GuildMarker> = parse_var("DISCORD_GUILD");

    let http = HttpClient::new(token.clone());
    let user_id = http.current_user().await?.model().await?.id;
    let tracks = load_tracks::get_tracks("music")?;

    if tracks.is_empty() {
        return Err("`music` folder must contain at least 1 song".into());
    }

    let (mut shard, state) = {
        let shard = Shard::new(ShardId::ONE, token, Intents::GUILD_VOICE_STATES);
        let tmap =
            TwilightMap::new(std::iter::once((shard.id().number(), shard.sender())).collect());
        let songbird = Songbird::twilight(Arc::new(tmap), user_id).into();
        (
            shard,
            Arc::new(State {
                http: Arc::new(http),
                songbird,
                vc,
                guild,
            }),
        )
    };

    let sender = shard.sender();
    let tasks = TaskTracker::new();

    let mut player_handle = tokio::spawn(play(state.clone(), tracks));

    let event_state = state.clone();
    let event_tasks = tasks.clone();
    let event_loop = tokio::spawn(async move {
        let state = event_state.clone();
        while let Some(event) = shard.next_event(EventTypeFlags::GUILD_VOICE_STATES).await {
            let event = match event {
                Ok(v) => v,
                Err(e) => {
                    error!(err = ?e, "Event fetch failed");
                    continue;
                }
            };
            let state2 = state.clone();
            if let Event::GatewayClose(Some(gc)) = &event {
                info!(
                    code = gc.code,
                    message = gc.reason.as_ref(),
                    "Got gateway close frame"
                );
                if gc.code == 1000 {
                    break;
                }
            }
            event_tasks.spawn(async move {
                state2.songbird.process(&event).await;
            });
        }
    });

    futures_util::future::select(pin!(vss::shutdown_signal()), &mut player_handle).await;

    info!("Leaving VC");
    state.songbird.remove(state.guild).await.ok();

    info!("Closing connection to discord");
    sender.close(CloseFrame::NORMAL).ok();

    info!("Waiting for event loop to exit");
    event_loop.await?;

    info!("Canceling player");
    player_handle.abort();

    info!("Waiting for handlers to shut down");
    tasks.close();
    tasks.wait().await;

    info!("Done, good day!");
    Ok(())
}

async fn play(
    state: Arc<State>,
    mut tracks: Box<[Song]>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    if let Err(source) = state.songbird.remove(state.guild).await {
        if !matches!(source, songbird::error::JoinError::NoCall) {
            error!(?source, "error joining call");
            return Err(Box::new(source));
        }
    }
    let call = match state.songbird.join(state.guild, state.vc).await {
        Ok(call) => call,
        Err(source) => {
            error!(?source, "error joining call");
            return Err(Box::new(source));
        }
    };

    loop {
        tracks.shuffle(&mut rand::rng());
        for song in &tracks {
            if let Err(source) = play_song(call.clone(), state.clone(), song).await {
                error!(?source, "error playing song");
            }
        }
        info!("Reached last song, restarting...");
    }
}

async fn play_song(
    call: Arc<Mutex<Call>>,
    state: Arc<State>,
    song: &Song,
) -> Result<(), Box<dyn Error + Send + Sync + 'static>> {
    let src: Input = song.data.clone().into();
    let content = format!("Now playing {}", song.meta);
    info!(
        name = &song.meta.title.as_ref(),
        album = &song.meta.album.as_ref(),
        artist = song.meta.artist.as_ref(),
        "now playing song"
    );
    state
        .http
        .create_message(state.vc)
        .content(&content)
        .await?;
    let handle = call.lock().await.play_input(src);
    while let Ok(v) = handle.get_info().await {
        if v.playing.is_done() {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Ok(())
}

fn get_var(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} required in the environment"))
}

fn parse_var<T>(name: &str) -> T
where
    T: FromStr,
    T::Err: std::fmt::Debug,
{
    get_var(name)
        .parse()
        .unwrap_or_else(|_| panic!("{name} must be a valid {}", std::any::type_name::<T>()))
}
