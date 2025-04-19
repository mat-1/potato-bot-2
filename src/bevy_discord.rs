//! A Bevy plugin for controlling a Discord bot.

use std::{num::NonZeroU64, sync::Arc};

use async_compat::Compat;
use bevy_tasks::{IoTaskPool, Task};
use futures_lite::future;
use log::warn;
use parking_lot::Mutex;
use tokio::sync::mpsc;
use twilight_cache_inmemory::{InMemoryCache, ResourceType};
pub use twilight_gateway::Intents;
use twilight_gateway::{error::ReceiveMessageError, Event, Shard, ShardId};
use twilight_gateway::{EventTypeFlags, StreamExt};
use twilight_http::{
    request::channel::reaction::RequestReactionType, response::marker::EmptyBody,
    Client as HttpClient, Response,
};
use twilight_model::channel::{message::AllowedMentions, Message};

use azalea::app::prelude::*;
use azalea::ecs::prelude::*;
use azalea::prelude::*;

pub mod recv {
    use azalea::ecs::prelude::*;
    use azalea::prelude::*;

    #[derive(Debug, Event)]
    pub struct MessageCreate(pub twilight_model::gateway::payload::incoming::MessageCreate);
}
pub mod send {
    use azalea::ecs::prelude::*;
    use azalea::prelude::*;

    #[derive(Debug, Event)]
    pub struct CreateMessage {
        pub channel_id: u64,
        pub content: String,
    }
    #[derive(Debug, Event)]
    pub struct CreateReaction {
        pub channel_id: u64,
        pub message_id: u64,
        pub emoji: char,
    }
}

pub struct DiscordPlugin {
    pub token: String,
    rx: Arc<Mutex<Option<mpsc::UnboundedReceiver<Result<Event, ReceiveMessageError>>>>>,
}

impl DiscordPlugin {
    pub async fn start(token: &str, intents: Intents) -> Self {
        let shard = Shard::new(ShardId::ONE, token.to_string(), intents);
        let (tx, rx) = mpsc::unbounded_channel();

        tokio::spawn(loop_get_next_events(shard, tx));

        Self {
            token: token.to_string(),
            rx: Arc::new(Mutex::new(Some(rx))),
        }
    }
}

impl Plugin for DiscordPlugin {
    fn build(&self, app: &mut App) {
        app.add_event::<recv::MessageCreate>()
            .add_event::<send::CreateMessage>()
            .add_event::<send::CreateReaction>()
            .add_systems(Update, handle_from_discord_events)
            .add_systems(
                Update,
                (
                    (
                        handle_create_message,
                        handle_create_message_response,
                        handle_create_reaction,
                    )
                        .after(handle_from_discord_events),
                    handle_empty_body_response,
                ),
            );

        let discord = Discord::new(self.token.clone(), self.rx.lock().take().unwrap());
        app.insert_resource(discord);
    }
}

impl Discord {
    pub fn new(
        token: String,
        rx: mpsc::UnboundedReceiver<Result<Event, ReceiveMessageError>>,
    ) -> Self {
        let http = Arc::new(HttpClient::new(token));

        let cache = InMemoryCache::builder()
            .resource_types(ResourceType::MESSAGE)
            .build();

        Discord { http, cache, rx }
    }
}

#[derive(Resource)]
pub struct Discord {
    pub http: Arc<HttpClient>,
    pub cache: InMemoryCache,
    rx: mpsc::UnboundedReceiver<Result<Event, ReceiveMessageError>>,
}

async fn loop_get_next_events(
    mut shard: Shard,
    tx: mpsc::UnboundedSender<Result<Event, ReceiveMessageError>>,
) {
    while let Some(event) = shard.next_event(EventTypeFlags::all()).await {
        // we do it like this because it has to run in the tokio runtime and
        // async_compat doesn't work for next_event
        if tx.send(event).is_err() {
            println!("couldn't send event to discord (probably because the receiver was dropped)");
            return;
        }
    }

    warn!("discord stream finished");
}

pub fn handle_from_discord_events(
    mut discord: ResMut<Discord>,
    mut message_create_events: EventWriter<recv::MessageCreate>,
) {
    while let Ok(event) = discord.rx.try_recv() {
        let event = match event {
            Ok(event) => event,
            Err(source) => {
                warn!("error receiving event {source}");
                continue;
            }
        };
        discord.cache.update(&event);
        match event {
            twilight_gateway::Event::MessageCreate(m) => {
                message_create_events.send(recv::MessageCreate(*m));
            }
            _ => {}
        }
    }
}

#[derive(Component)]
pub struct DiscordResponseTask<T>(Task<Result<Response<T>, twilight_http::Error>>);

fn handle_create_message(
    mut commands: Commands,
    discord: Res<Discord>,
    mut events: EventReader<send::CreateMessage>,
) {
    let task_pool = IoTaskPool::get();

    for event in events.read() {
        let content = event.content.clone();

        let channel_id = event.channel_id;

        let http = discord.http.clone();

        let task = task_pool.spawn(Compat::new(async move {
            http.create_message(NonZeroU64::try_from(channel_id).unwrap().into())
                .allowed_mentions(Some(&AllowedMentions::default()))
                .content(&content)
                .await
        }));
        commands.spawn(DiscordResponseTask(task));
    }
}
fn handle_create_message_response(
    mut commands: Commands,
    mut query: Query<(Entity, &mut DiscordResponseTask<Message>)>,
) {
    for (entity, mut response) in &mut query {
        let Some(_result) = future::block_on(future::poll_once(&mut response.0)) else {
            continue;
        };
        commands
            .entity(entity)
            .remove::<DiscordResponseTask<Message>>();
    }
}

pub fn handle_create_reaction(
    mut commands: Commands,
    discord: Res<Discord>,
    mut events: EventReader<send::CreateReaction>,
) {
    let task_pool = IoTaskPool::get();

    for event in events.read() {
        let channel_id = event.channel_id;
        let message_id = event.message_id;
        let emoji = event.emoji;

        let http = discord.http.clone();

        let task = task_pool.spawn(Compat::new(async move {
            http.create_reaction(
                NonZeroU64::try_from(channel_id).unwrap().into(),
                NonZeroU64::try_from(message_id).unwrap().into(),
                &RequestReactionType::Unicode {
                    name: &emoji.to_string(),
                },
            )
            .await
        }));
        commands.spawn(DiscordResponseTask(task));
    }
}
fn handle_empty_body_response(
    mut commands: Commands,
    mut query: Query<(Entity, &mut DiscordResponseTask<EmptyBody>)>,
) {
    for (entity, mut response) in &mut query {
        let Some(_result) = future::block_on(future::poll_once(&mut response.0)) else {
            continue;
        };
        commands
            .entity(entity)
            .remove::<DiscordResponseTask<EmptyBody>>();
    }
}
