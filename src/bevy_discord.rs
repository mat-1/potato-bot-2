//! A Bevy plugin for controlling a Discord bot.

use std::{num::NonZeroU64, sync::Arc};

use async_compat::Compat;
use azalea::app::{App, Plugin};
use azalea::ecs::schedule::IntoSystemConfigs;
use azalea::ecs::{
    component::Component,
    entity::Entity,
    event::{EventReader, EventWriter},
    system::{Commands, Query, Res, ResMut, Resource},
};
use azalea::prelude::bevy_ecs;
use bevy_tasks::{IoTaskPool, Task};
use futures_lite::future;
use log::{error, warn};
use tokio::sync::mpsc;
use twilight_cache_inmemory::{InMemoryCache, ResourceType};
pub use twilight_gateway::Intents;
use twilight_gateway::{error::ReceiveMessageError, Event, Shard, ShardId};
use twilight_http::{
    request::channel::reaction::RequestReactionType, response::marker::EmptyBody,
    Client as HttpClient, Response,
};
use twilight_model::channel::{message::AllowedMentions, Message};
use twilight_validate::message::MessageValidationError;

pub mod recv {
    pub use twilight_gateway::Event;
    pub use twilight_model::gateway::payload::incoming::MessageCreate;
}
pub mod send {
    #[derive(Debug)]
    pub struct CreateMessage {
        pub channel_id: u64,
        pub content: String,
    }
    #[derive(Debug)]
    pub struct CreateReaction {
        pub channel_id: u64,
        pub message_id: u64,
        pub emoji: char,
    }
}

#[derive(Clone)]
pub struct DiscordPlugin {
    pub token: String,
    pub intents: Intents,
}
impl Plugin for DiscordPlugin {
    fn build(&self, app: &mut App) {
        app.add_event::<recv::MessageCreate>()
            .add_event::<send::CreateMessage>()
            .add_event::<send::CreateReaction>()
            .add_system(handle_from_discord_events)
            .add_systems(
                (
                    handle_create_message,
                    handle_create_message_response,
                    handle_create_reaction,
                )
                    .after(handle_from_discord_events),
            )
            .add_system(handle_empty_body_response);

        app.insert_resource(Discord::new(self.token.clone(), self.intents));
    }
}

impl Discord {
    pub fn new(token: String, intents: Intents) -> Self {
        let shard = Shard::new(ShardId::ONE, token.clone(), intents);
        let http = Arc::new(HttpClient::new(token));
        let cache = InMemoryCache::builder()
            .resource_types(ResourceType::MESSAGE)
            .build();

        let (tx, rx) = mpsc::unbounded_channel();

        Discord {
            http,
            cache,
            rx,

            shard: Some(shard),
            task: None,
            tx: Some(tx),
        }
    }
}

#[derive(Resource)]
pub struct Discord {
    pub http: Arc<HttpClient>,
    pub cache: InMemoryCache,
    rx: mpsc::UnboundedReceiver<Result<Event, ReceiveMessageError>>,

    shard: Option<Shard>,
    task: Option<Task<()>>,
    tx: Option<mpsc::UnboundedSender<Result<Event, ReceiveMessageError>>>,
}

async fn loop_get_next_events(
    mut shard: Shard,
    tx: mpsc::UnboundedSender<Result<Event, ReceiveMessageError>>,
) {
    loop {
        // we do it like this because it has to run in the tokio runtime and
        // async_compat doesn't work for next_event
        let event = shard.next_event().await;
        if tx.send(event).is_err() {
            println!("couldn't send event to discord (probably because the receiver was dropped)");
            return;
        }
    }
}

pub fn handle_from_discord_events(
    mut discord: ResMut<Discord>,
    mut message_create_events: EventWriter<recv::MessageCreate>,
) {
    let pool = IoTaskPool::get();
    if discord.task.is_none() {
        discord.task = Some(pool.spawn(Compat::new(loop_get_next_events(
            discord.shard.take().unwrap(),
            discord.tx.take().unwrap(),
        ))));
    }
    let mut discord_task = discord.task.as_mut().unwrap();
    future::block_on(future::poll_once(&mut discord_task));
    while let Ok(event) = discord.rx.try_recv() {
        let event = match event {
            Ok(event) => event,
            Err(source) => {
                if source.is_fatal() {
                    error!("fatal error receiving event {source}");
                    continue;
                }
                warn!("error receiving event {source}");
                continue;
            }
        };
        discord.cache.update(&event);
        match event {
            recv::Event::MessageCreate(m) => message_create_events.send(*m),
            _ => {}
        }
    }
}

#[derive(Component)]
pub struct DiscordResponseTask<T>(
    Task<Result<Result<Response<T>, twilight_http::Error>, MessageValidationError>>,
);

fn handle_create_message(
    mut commands: Commands,
    discord: Res<Discord>,
    mut events: EventReader<send::CreateMessage>,
) {
    let task_pool = IoTaskPool::get();

    for event in events.iter() {
        let content = event.content.clone();
        let channel_id = event.channel_id;

        let http = discord.http.clone();

        let task = task_pool.spawn(Compat::new(async move {
            match http
                .create_message(NonZeroU64::try_from(channel_id).unwrap().into())
                .allowed_mentions(Some(&AllowedMentions::default()))
                .content(&content)
            {
                Ok(created_message) => Ok(created_message.await),
                Err(e) => Err(e),
            }
        }));
        commands.spawn(DiscordResponseTask(task));
    }
}
fn handle_create_message_response(
    mut commands: Commands,
    mut query: Query<(Entity, &mut DiscordResponseTask<Message>)>,
) {
    for (entity, mut response) in &mut query {
        let Some(_result) = future::block_on(future::poll_once(&mut response.0)) else { continue };
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

    for event in events.iter() {
        let channel_id = event.channel_id;
        let message_id = event.message_id;
        let emoji = event.emoji;

        let http = discord.http.clone();

        let task = task_pool.spawn(Compat::new(async move {
            Ok(http
                .create_reaction(
                    NonZeroU64::try_from(channel_id).unwrap().into(),
                    NonZeroU64::try_from(message_id).unwrap().into(),
                    &RequestReactionType::Unicode {
                        name: &emoji.to_string(),
                    },
                )
                .await)
        }));
        commands.spawn(DiscordResponseTask(task));
    }
}
fn handle_empty_body_response(
    mut commands: Commands,
    mut query: Query<(Entity, &mut DiscordResponseTask<EmptyBody>)>,
) {
    for (entity, mut response) in &mut query {
        let Some(_result) = future::block_on(future::poll_once(&mut response.0)) else { continue };
        commands
            .entity(entity)
            .remove::<DiscordResponseTask<EmptyBody>>();
    }
}
