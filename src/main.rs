use futures::StreamExt;
use songbird::{input::Input, shards::TwilightMap, Call, Songbird};
use std::{
    env,
    error::Error,
    fmt::Display,
    sync::{atomic::AtomicBool, Arc},
};
use tokio::sync::Mutex;
use twilight_gateway::{
    stream::{self, ShardEventStream},
    CloseFrame, Intents, MessageSender, Shard,
};
use twilight_http::Client as HttpClient;
use twilight_model::id::{
    marker::{ChannelMarker, GuildMarker},
    Id,
};

type State = Arc<StateRef>;

#[derive(serde::Deserialize, Debug, Clone)]
struct SongMetadata {
    artist: String,
    name: String,
    album: String,
}

impl Display for SongMetadata {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} by {} ({})", self.name, self.artist, self.album)
    }
}

#[derive(Debug, Clone)]
struct Song {
    data: Vec<u8>,
    meta: SongMetadata,
}

#[derive(Debug)]
struct StateRef {
    http: HttpClient,
    songbird: Songbird,
    songs: Arc<Vec<Song>>,
    vc: Id<ChannelMarker>,
    guild: Id<GuildMarker>,
    shutdown: Arc<AtomicBool>,
    senders: Vec<MessageSender>,
}

impl StateRef {
    pub fn shutdown(&self) {
        tracing::warn!("Shutting down...");
        self.shutdown
            .store(true, std::sync::atomic::Ordering::Relaxed);
        for sender in &self.senders {
            sender.close(CloseFrame::NORMAL).ok();
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync + 'static>> {
    dotenvy::dotenv().ok();
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "hypersonic=info");
    }
    // Initialize the tracing subscriber.
    tracing_subscriber::fmt::init();

    let (mut shards, state) = {
        let token = env::var("DISCORD_TOKEN").expect("Missing environment variable DISCORD_TOKEN");
        let vc_str = env::var("DISCORD_VC").expect("Missing environment variable DISCORD_VC");
        let guild_str =
            env::var("DISCORD_GUILD").expect("Missing environment variable DISCORD_GUILD");
        let vc_id: Id<ChannelMarker> = vc_str
            .parse()
            .expect("Expected valid integer for DISCORD_VC");

        let guild_id: Id<GuildMarker> = guild_str
            .parse()
            .expect("Expected valid integer for DISCORD_GUILD");
        let http = HttpClient::new(token.clone());
        let user_id = http.current_user().await?.model().await?.id;

        let intents = Intents::GUILD_VOICE_STATES;
        let config = twilight_gateway::Config::new(token.clone(), intents);
        let tracks = {
            let metadata_list: Vec<SongMetadata> = serde_json::from_slice(
                &std::fs::read("./music/meta.json").expect("Failed to read ./music/meta.json"),
            )
            .expect("Failed to deserialize ./music/meta.json");
            let mut tracks: Vec<Song> = Vec::with_capacity(metadata_list.len());
            for meta in metadata_list {
                let file_name = format!("./music/{}.mp3", meta.name);
                let data = std::fs::read(&file_name)
                    .unwrap_or_else(|e| panic!("Failed to read {file_name}: {e:?}"));
                tracks.push(Song { meta, data });
            }
            tracks
        };
        let shards: Vec<Shard> =
            stream::create_recommended(&http, config, |_, builder| builder.build())
                .await?
                .collect();

        let tmap = TwilightMap::new(
            shards
                .iter()
                .map(|s| (s.id().number(), s.sender()))
                .collect(),
        );
        let senders: Vec<MessageSender> = shards.iter().map(|v| v.sender()).collect();
        let songbird = Songbird::twilight(Arc::new(tmap), user_id);
        (
            shards,
            Arc::new(StateRef {
                http,
                songbird,
                songs: Arc::new(tracks),
                vc: vc_id,
                guild: guild_id,
                senders,
                shutdown: Arc::new(AtomicBool::new(false)),
            }),
        )
    };
    let mut stream = ShardEventStream::new(shards.iter_mut());
    let state_ctrlc = state.clone();
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.unwrap();
        state_ctrlc.clone().shutdown();
    });
    tokio::spawn(play(state.clone()));
    loop {
        let event = match stream.next().await {
            Some((_, Ok(event))) => event,
            Some((_, Err(source))) => {
                tracing::warn!(?source, "error receiving event");

                if source.is_fatal() {
                    break;
                }

                continue;
            }
            None => break,
        };
        let state = state.clone();
        state.songbird.process(&event).await;
        if state.shutdown.load(std::sync::atomic::Ordering::Relaxed) {
            state.songbird.leave(state.guild).await.ok();
            break;
        }
    }

    Ok(())
}

async fn play(state: State) {
    if let Err(e) = state.songbird.remove(state.guild).await {
        if !matches!(e, songbird::error::JoinError::NoCall) {
            tracing::error!("{e:?}");
            state.shutdown();
            return;
        }
    };
    if state.songs.is_empty() {
        tracing::error!("Songs list empty!");
        state.shutdown();
        return;
    };
    let call = match state.songbird.join(state.guild, state.vc).await {
        Ok(call) => call,
        Err(e) => {
            tracing::error!("{e:?}");
            state.shutdown();
            return;
        }
    };
    loop {
        for song in &*state.songs {
            if let Err(e) = play_song(call.clone(), state.clone(), song).await {
                tracing::error!("{e:?}");
            }
        }
        tracing::info!("Reached last song, restarting...");
    }
}

async fn play_song(
    call: Arc<Mutex<Call>>,
    state: State,
    song: &Song,
) -> Result<(), Box<dyn Error + Send + Sync + 'static>> {
    let src: Input = song.data.clone().into();
    let content = format!("Now playing {}", song.meta);
    tracing::info!("{}", content);
    state
        .http
        .create_message(state.vc)
        .content(&content)?
        .await?;
    let handle = call.lock().await.play_input(src);
    while let Ok(v) = handle.get_info().await {
        if v.playing.is_done() {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    Ok(())
}
