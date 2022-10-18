#![feature(async_closure)]

use parking_lot::Mutex;
use serenity::futures::future::{self,BoxFuture};
use serenity::model::channel::Message;
use serenity::model::id::ChannelId;
use serenity::{async_trait, prelude::*};
use std::env;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;

struct Handler;

struct MessagesQueuedToMinecraft;
impl TypeMapKey for MessagesQueuedToMinecraft {
    type Value = Arc<Mutex<Vec<QueuedMessage>>>;
}

// https://stackoverflow.com/a/66070319
pub trait QueuedMessageCallback: Send {
    fn call(self: Box<Self>, success: bool) -> BoxFuture<'static, ()>;
}
impl<T, F> QueuedMessageCallback for T
where
    T: FnOnce(bool) -> F + Send,
    F: Future<Output = ()> + 'static + Send,
{
    fn call(self: Box<Self>, success: bool) -> BoxFuture<'static, ()> {
        Box::pin(self(success))
    }
}
pub struct QueuedMessage {
    pub content: String,
    pub callback: Box<dyn QueuedMessageCallback>,
}
struct MessagesQueuedToDiscord;
impl TypeMapKey for MessagesQueuedToDiscord {
    type Value = Arc<Mutex<Vec<String>>>;
}

#[async_trait]
impl EventHandler for Handler {
    async fn message(&self, ctx: Context, msg: Message) {
        if msg.author.bot {
            return;
        };
        let discord_channel_id: u64 = env::var("DISCORD_CHANNEL_ID").unwrap().parse().unwrap();
        if msg.channel_id != discord_channel_id {
            return;
        };

        let queued_to_minecraft = {
            let data_read = ctx.data.read().await;
            data_read
                .get::<MessagesQueuedToMinecraft>()
                .expect("Expected MessagesQueuedToMinecraft in TypeMap.")
                .clone()
        };

        let message_content = msg.content.clone();
        let message_content = format!(
            "{}#{}: {}",
            msg.author.name, msg.author.discriminator, message_content
        );

        if !message_legal_to_minecraft(&message_content) {
            if let Err(e) = msg.react(&ctx, '🚫').await {
                eprintln!("Couldn't react with thumbsup/thumbsdown: {}", e);
            }
        }

        let callback = async move |success: bool| {
            if let Err(e) = msg.react(&ctx, if success { '👍' } else { '👎' }).await {
                eprintln!("Couldn't react with thumbsup/thumbsdown: {}", e);
            }
        };

        queued_to_minecraft.lock().push(QueuedMessage {
            content: message_content,
            callback: Box::new(callback),
        });
    }
}

#[derive(Default)]
struct State {
    pub messages_queued_to_minecraft: Arc<Mutex<Vec<QueuedMessage>>>,
    pub messages_queued_to_discord: Arc<Mutex<Vec<String>>>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();
    dotenv::dotenv().expect("Failed to load .env file");

    let account =
        azalea::Account::microsoft(&env::var("EMAIL").expect("Expected EMAIL in env")).await?;
    let token = env::var("DISCORD_TOKEN").expect("Expected DISCORD_TOKEN in env");

    let discord_channel_id: u64 = env::var("DISCORD_CHANNEL_ID").unwrap().parse().unwrap();

    let intents = GatewayIntents::GUILD_MESSAGES
        | GatewayIntents::DIRECT_MESSAGES
        | GatewayIntents::MESSAGE_CONTENT;

    // Create a new instance of the Client, logging in as a bot. This will
    // automatically prepend your bot token with "Bot ", which is a requirement
    // by Discord for bot users.
    let discord_client = Client::builder(&token, intents)
        .event_handler(Handler)
        .await
        .expect("Err creating client");

    let messages_queued_to_minecraft = Arc::new(Mutex::new(Vec::default()));
    let messages_queued_to_discord = Arc::new(Mutex::new(Vec::default()));

    {
        let mut data = discord_client.data.write().await;

        data.insert::<MessagesQueuedToMinecraft>(messages_queued_to_minecraft.clone());
        data.insert::<MessagesQueuedToDiscord>(messages_queued_to_discord.clone());
    }

    // move `messages_queued_to_discord` into the future without taking ownership of the original
    let _messages_queued_to_discord = messages_queued_to_discord.clone();
    tokio::spawn(async move {
        let messages_queued_to_discord = _messages_queued_to_discord;
        loop {
            // send minecraft messages back to discord every tick
            sleep(Duration::from_millis(50)).await;

            let messages_queued_to_discord = {
                let mut messages_queued_to_discord = messages_queued_to_discord.lock();
                messages_queued_to_discord
                    .drain(..)
                    .collect::<Vec<String>>()
            };
            if !messages_queued_to_discord.is_empty() {
                let channel_id = ChannelId::from(discord_channel_id);
                let mut sending_message = String::new();
                for message in messages_queued_to_discord {
                    if message.len() > 2000 {
                        // hopefully doesn't happen
                        eprintln!("Minecraft message was sent that was over 2000 characters!");
                        continue;
                    }
                    // adding this message would make it longer than the limit, so send now
                    if (message.len() + sending_message.len()) >= 2000 {
                        if let Err(e) = channel_id
                            .say(&discord_client.cache_and_http.http, &sending_message)
                            .await
                        {
                            eprintln!("Couldn't send message to Discord: {:?}", e);
                        };
                        sending_message.clear();
                    }
                    sending_message.push_str(&message);
                    sending_message.push('\n');
                }
                // channel_id.say(&discord_client.cache_and_http.http, );
            }
        }
    });

    azalea::start(azalea::Options {
        account,
        address: "localhost",
        state: Arc::new(Mutex::new(State {
            messages_queued_to_minecraft,
            messages_queued_to_discord,
        })),
        plugins: vec![],
        handle: mc_handle,
    })
    .await
    .unwrap();

    Ok(())
}

async fn mc_handle(
    mut bot: azalea::Client,
    event: Arc<azalea::Event>,
    state: Arc<Mutex<State>>,
) -> anyhow::Result<()> {
    match &*event {
        azalea::Event::Login => {
            bot.chat("Hello world").await?;
        }
        azalea::Event::Tick => {
            let messages_queued_to_minecraft = {
                let state_lock = state.lock();
                let messages_queued_to_minecraft =
                    &mut state_lock.messages_queued_to_minecraft.lock();
                messages_queued_to_minecraft
                    .drain(..)
                    .collect::<Vec<QueuedMessage>>()
            };
            if !messages_queued_to_minecraft.is_empty() {
                let mut futures = vec![];
                for message in messages_queued_to_minecraft {
                    futures.push(async {
                        let message = message;
                        let message_content = message.content.clone();
                        let chat_result = bot.chat(&message_content).await;
                        (message.callback).call(chat_result.is_ok()).await;
                    });
                }
                future::join_all(futures).await;
            }
            bot.walk(azalea::MoveDirection::ForwardLeft);
        }
        azalea::Event::Chat(m) => {
            let message_string = m.message().to_string();
            let state_lock = state.lock();
            let mut messages_queued_to_discord = state_lock.messages_queued_to_discord.lock();
            messages_queued_to_discord.push(message_string);
        }
        _ => {}
    }

    Ok(())
}

/// Whether this message can be sent to Minecraft without the server kicking us.
fn message_legal_to_minecraft(message: &str) -> bool {
    if message.starts_with('/') {
        return false;
    }
    if message.len() > 256 {
        return false;
    }
    for char in message.chars() {
        if matches!(char, '\x00'..='\x1F' | '\x7F' | '§') {
            return false;
        }
    }

    return true;
}
