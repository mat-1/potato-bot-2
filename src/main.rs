#![feature(async_closure)]

use azalea_protocol::packets::game::serverbound_client_command_packet::ServerboundClientCommandPacket;
use parking_lot::Mutex;
use serenity::futures::future::{self, BoxFuture};
use serenity::model::channel::Message;
use serenity::model::id::ChannelId;
use serenity::{async_trait, prelude::*};
use std::env;
use std::future::Future;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
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

        println!("got discord message");
        let queued_to_minecraft = {
            let data_read = ctx.data.read().await;
            data_read
                .get::<MessagesQueuedToMinecraft>()
                .expect("Expected MessagesQueuedToMinecraft in TypeMap.")
                .clone()
        };

        let message_content = msg.content.clone();
        let message_content = format!(
            "{}#{:0>4}: {}",
            msg.author.name, msg.author.discriminator, message_content
        );

        if !message_legal_to_minecraft(&message_content) {
            if let Err(e) = msg.react(&ctx, '🚫').await {
                eprintln!("Couldn't react with thumbsup/thumbsdown: {}", e);
            }
            return;
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

#[derive(Default, Clone)]
struct State {
    pub messages_queued_to_minecraft: Arc<Mutex<Vec<QueuedMessage>>>,
    pub messages_queued_to_discord: Arc<Mutex<Vec<String>>>,

    pub chat_spam_tick_count: Arc<AtomicUsize>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenv::dotenv().expect("Failed to load .env file");
    env_logger::init();

    let account =
        azalea::Account::microsoft(&env::var("EMAIL").expect("Expected EMAIL in env")).await?;
    let token = env::var("DISCORD_TOKEN").expect("Expected DISCORD_TOKEN in env");

    let discord_channel_id: u64 = env::var("DISCORD_CHANNEL_ID").unwrap().parse().unwrap();

    let intents = GatewayIntents::non_privileged() | GatewayIntents::MESSAGE_CONTENT;

    // Create a new instance of the Client, logging in as a bot. This will
    // automatically prepend your bot token with "Bot ", which is a requirement
    // by Discord for bot users.
    let mut discord_client = Client::builder(&token, intents)
        .event_handler(Handler)
        .await
        .expect("Err creating client");

    let messages_queued_to_minecraft = Arc::new(Mutex::new(Vec::default()));
    let messages_queued_to_discord = Arc::new(Mutex::new(Vec::default()));

    let discord_client_data = discord_client.data.clone();
    let discord_client_cache_and_http = discord_client.cache_and_http.clone();

    {
        let mut data = discord_client_data.write().await;

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
                        if let Err(e) = send_message(
                            &channel_id,
                            &discord_client_cache_and_http.http,
                            &sending_message,
                        )
                        .await
                        {
                            eprintln!("Couldn't send message to Discord: {:?}", e);
                        };
                        sending_message.clear();
                    }
                    sending_message.push_str(&message);
                    sending_message.push('\n');
                }
                if let Err(e) = send_message(
                    &channel_id,
                    &discord_client_cache_and_http.http,
                    &sending_message,
                )
                .await
                {
                    eprintln!("Couldn't send message to Discord: {:?}", e);
                };
            }
        }
    });

    tokio::spawn(async move {
        if let Err(why) = discord_client.start().await {
            println!("Discord client error: {:?}", why);
        };
    });

    loop {
        let error = azalea::start(azalea::Options {
            account: account.clone(),
            address: &env::var("SERVER_IP").expect("Expected SERVER_IP in env")[..],
            state: State {
                messages_queued_to_minecraft: messages_queued_to_minecraft.clone(),
                messages_queued_to_discord: messages_queued_to_discord.clone(),
                chat_spam_tick_count: Arc::new(AtomicUsize::new(0)),
            },
            plugins: azalea::plugins![],
            handle: mc_handle,
        })
        .await;
        eprintln!("{:?}", error);
        sleep(Duration::from_secs(4)).await;
    }

    // Ok(())
}

async fn mc_handle(bot: azalea::Client, event: azalea::Event, state: State) -> anyhow::Result<()> {
    match event {
        azalea::Event::Login => {}
        azalea::Event::Death(_) => {
            bot.write_packet(ServerboundClientCommandPacket {
                action: azalea_protocol::packets::game::serverbound_client_command_packet::Action::PerformRespawn,
            }.get()).await?;
        }
        azalea::Event::Tick => {
            // decrease the chat_spam_tick_count every tick (unless it's 0)
            let _ =
                state
                    .chat_spam_tick_count
                    .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |x| {
                        if x > 0 {
                            Some(x - 1)
                        } else {
                            None
                        }
                    });

            let messages_queued_to_minecraft = {
                let messages_queued_to_minecraft = &mut state.messages_queued_to_minecraft.lock();
                // the 100 is actually 200 in vanilla, but i chose 100 to make sure it doesn't go over
                messages_queued_to_minecraft
                    .drain(..((100 - state.chat_spam_tick_count.load(Ordering::SeqCst)) / 20))
                    .collect::<Vec<QueuedMessage>>()
            };
            if !messages_queued_to_minecraft.is_empty() {
                let mut futures = vec![];
                state
                    .chat_spam_tick_count
                    .fetch_add(messages_queued_to_minecraft.len() * 20, Ordering::SeqCst);
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
            // bot.walk(azalea::MoveDirection::ForwardLeft);
        }
        azalea::Event::Chat(m) => {
            println!("Got Minecraft chat packet: {}", m.message().to_ansi());
            let message_string = m.message().to_string();
            if message_string.starts_with("<matdoesdev> ")
                || message_string == "death.fell.accident.water"
            {
                return Ok(());
            }
            let content_part = message_string
                .splitn(2, "> ")
                .nth(1)
                .unwrap_or(&message_string);
            if content_part.starts_with("/skill") {
                // spam
                return Ok(());
            }
            let mut messages_queued_to_discord = state.messages_queued_to_discord.lock();
            messages_queued_to_discord.push(message_string);
            println!(
                "messages_queued_to_discord: {:?}",
                messages_queued_to_discord
            );
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

async fn send_message(
    channel_id: &ChannelId,
    http: &Arc<serenity::http::Http>,
    message: &str,
) -> Result<(), serenity::Error> {
    channel_id
        .send_message(&http, |m| {
            m.content(message).allowed_mentions(|am| am.empty_parse())
        })
        .await?;
    Ok(())
}
