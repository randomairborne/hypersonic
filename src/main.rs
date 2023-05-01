use futures::StreamExt;
use songbird::{input::Input, shards::TwilightMap, Call, Songbird};
use std::{
    env,
    error::Error,
    sync::{atomic::AtomicBool, Arc},
};
use tokio::sync::Mutex;
use twilight_gateway::{
    stream::{self, ShardEventStream},
    Intents, Shard,
};
use twilight_http::Client as HttpClient;
use twilight_model::id::{
    marker::{ChannelMarker, GuildMarker},
    Id,
};

type State = Arc<StateRef>;

#[derive(Debug)]
struct StateRef {
    http: HttpClient,
    songbird: Songbird,
    songs: Arc<Vec<Vec<u8>>>,
    vc: Id<ChannelMarker>,
    guild: Id<GuildMarker>,
    shutdown: Arc<AtomicBool>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync + 'static>> {
    dotenvy::dotenv().ok();
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
            let files_iter = std::fs::read_dir("music").expect("Failed to list music folder");
            let mut tracks: Vec<Vec<u8>> = Vec::new();
            for file in files_iter {
                let Ok(file) = file else { break; };
                let data =
                    std::fs::read(file.path()).expect("there should be a file, but there wasn't");
                tracks.push(data);
            }
            tracks
        };
        let shards: Vec<Shard> =
            stream::create_recommended(&http, config, |_, builder| builder.build())
                .await?
                .collect();

        let senders = TwilightMap::new(
            shards
                .iter()
                .map(|s| (s.id().number(), s.sender()))
                .collect(),
        );

        let songbird = Songbird::twilight(Arc::new(senders), user_id);

        (
            shards,
            Arc::new(StateRef {
                http,
                songbird,
                songs: Arc::new(tracks),
                vc: vc_id,
                guild: guild_id,
                shutdown: Arc::new(AtomicBool::new(false)),
            }),
        )
    };
    let mut stream = ShardEventStream::new(shards.iter_mut());
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
            state.songbird.remove(state.guild).await.ok();
            break;
        }
    }

    Ok(())
}

async fn play(state: State) {
    if let Err(e) = state.songbird.remove(state.guild).await {
        if !matches!(e, songbird::error::JoinError::NoCall) {
            tracing::error!("{e:?}");
            state
                .shutdown
                .store(true, std::sync::atomic::Ordering::Relaxed)
        }
    };
    if state.songs.is_empty() {
        tracing::error!("Songs list empty!");
        state
            .shutdown
            .store(true, std::sync::atomic::Ordering::Relaxed)
    };
    let call = match state.songbird.join(state.guild, state.vc).await {
        Ok(call) => call,
        Err(e) => {
            tracing::error!("{e:?}");
            state
                .shutdown
                .store(true, std::sync::atomic::Ordering::Relaxed);
            return;
        }
    };
    let mut current_index = 0;
    loop {
        if let Err(e) = play_idx(call.clone(), state.clone(), current_index).await {
            tracing::error!("{e:?}");
        }
        current_index += 1;
        if current_index >= state.songs.len() {
            current_index = 0;
        }
    }
}

async fn play_idx(
    call: Arc<Mutex<Call>>,
    state: State,
    idx: usize,
) -> Result<(), Box<dyn Error + Send + Sync + 'static>> {
    let mut src: Input = state.songs[idx].clone().into();
    let content = if let Ok(metadata) = src.aux_metadata().await {
        format!(
            "Playing **{:?}** by **{:?}**",
            metadata.track.as_ref().unwrap_or(&"<UNKNOWN>".to_string()),
            metadata.artist.as_ref().unwrap_or(&"<UNKNOWN>".to_string()),
        )
    } else {
        "Playing a song by someone".to_string()
    };
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
