#![feature(async_closure)]

use azalea::Account;

mod azalea_avoid_chat_kick;
mod azalea_bridge;
mod azalea_discord_bridge;
mod bevy_discord;

use azalea::prelude::*;
use azalea_protocol::packets::game::serverbound_client_command_packet::ServerboundClientCommandPacket;
use std::env;
use std::time::Duration;
use tokio::time::sleep;
use twilight_gateway::Intents;

use crate::azalea_avoid_chat_kick::AvoidKickPlugin;
use crate::azalea_discord_bridge::DiscordBridgePlugin;
use crate::bevy_discord::DiscordPlugin;

// #[derive(Component)]
// struct MessagesQueuedToMinecraft(pub Arc<Mutex<Vec<QueuedMessage>>>);

// pub struct QueuedMessage {
//     pub content: String,
// }
// struct MessagesQueuedToDiscord(pub Arc<Mutex<Vec<String>>>);

// struct Handler;

// fn handle_message_from_discord() {
//     if msg.author.bot {
//         return;
//     };
//     let discord_channel_id: u64 = env::var("DISCORD_CHANNEL_ID").unwrap().parse().unwrap();
//     if msg.channel_id != discord_channel_id {
//         return;
//     };

//     println!("got discord message");
//     let queued_to_minecraft = {
//         let data_read = ctx.data.read().await;
//         data_read
//             .get::<MessagesQueuedToMinecraft>()
//             .expect("Expected MessagesQueuedToMinecraft in TypeMap.")
//             .clone()
//     };

//     let message_content = msg.content.clone();
//     let message_content = format!(
//         "{}#{:0>4}: {}",
//         msg.author.name, msg.author.discriminator, message_content
//     );

//     if !message_legal_to_minecraft(&message_content) {
//         if let Err(e) = msg.react(&ctx, '🚫').await {
//             eprintln!("Couldn't react with thumbsup/thumbsdown: {}", e);
//         }
//         return;
//     }

//     let callback = async move |success: bool| {
//         if let Err(e) = msg.react(&ctx, if success { '👍' } else { '👎' }).await {
//             eprintln!("Couldn't react with thumbsup/thumbsdown: {}", e);
//         }
//     };

//     queued_to_minecraft.lock().push(QueuedMessage {
//         content: message_content,
//         callback: Box::new(callback),
//     });
// }

// #[derive(Clone)]
// pub struct RecentMessage {
//     pub content: String,
//     /// The number of times the message was sent. 1 if the message was sent once.
//     pub sent_count: usize,
//     pub sent_at: Instant,
// }

#[derive(Component, Default, Clone)]
struct State {
    //     pub messages_queued_to_minecraft: Arc<Mutex<Vec<QueuedMessage>>>,
    //     pub messages_queued_to_discord: Arc<Mutex<Vec<String>>>,

    //     pub chat_spam_tick_count: Arc<AtomicUsize>,

    //     /// Last is most recent. Only stores last 5 messages sent, if a message is
    //     /// re-sent then it gets removed and re-added with the new count.
    //     pub recent_messages: Arc<Mutex<VecDeque<RecentMessage>>>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenv::dotenv().expect("Failed to load .env file");
    env_logger::init();

    {
        use parking_lot::deadlock;
        use std::thread;
        use std::time::Duration;

        // Create a background thread which checks for deadlocks every 10s
        thread::spawn(move || loop {
            thread::sleep(Duration::from_secs(10));
            let deadlocks = deadlock::check_deadlock();
            if deadlocks.is_empty() {
                continue;
            }

            println!("{} deadlocks detected", deadlocks.len());
            for (i, threads) in deadlocks.iter().enumerate() {
                println!("Deadlock #{i}");
                for t in threads {
                    println!("Thread Id {:#?}", t.thread_id());
                    println!("{:#?}", t.backtrace());
                }
            }
        });
    }

    let account = if let Ok(email) = env::var("EMAIL") {
        Account::microsoft(&email).await?
    } else {
        eprintln!("No EMAIL in env, defaulting to offline-mode.");
        Account::offline("potatobot")
    };

    //     let messages_queued_to_minecraft = Arc::new(Mutex::new(Vec::default()));
    //     let messages_queued_to_discord = Arc::new(Mutex::new(Vec::default()));

    //     let discord_client_data = discord_client.data.clone();
    //     let discord_client_cache_and_http = discord_client.cache_and_http.clone();

    //     {
    //         let mut data = discord_client_data.write().await;

    //         data.insert::<MessagesQueuedToMinecraft>(messages_queued_to_minecraft.clone());
    //         data.insert::<MessagesQueuedToDiscord>(messages_queued_to_discord.clone());
    //     }

    //     // move `messages_queued_to_discord` into the future without taking ownership of the original
    //     let _messages_queued_to_discord = messages_queued_to_discord.clone();
    //     tokio::spawn(async move {
    //         let messages_queued_to_discord = _messages_queued_to_discord;
    //         loop {
    //             // send minecraft messages back to discord every tick
    //             sleep(Duration::from_millis(50)).await;

    //             let messages_queued_to_discord = {
    //                 let mut messages_queued_to_discord = messages_queued_to_discord.lock();
    //                 messages_queued_to_discord
    //                     .drain(..)
    //                     .collect::<Vec<String>>()
    //             };
    //             if !messages_queued_to_discord.is_empty() {
    //                 let channel_id = ChannelId::from(discord_channel_id);
    //                 let mut sending_message = String::new();
    //                 for message in messages_queued_to_discord {
    //                     if message.len() > 2000 {
    //                         // hopefully doesn't happen
    //                         eprintln!("Minecraft message was sent that was over 2000 characters!");
    //                         continue;
    //                     }
    //                     // adding this message would make it longer than the limit, so send now
    //                     if (message.len() + sending_message.len()) >= 2000 {
    //                         if let Err(e) = send_message(
    //                             &channel_id,
    //                             &discord_client_cache_and_http.http,
    //                             &sending_message,
    //                         )
    //                         .await
    //                         {
    //                             eprintln!("Couldn't send message to Discord: {:?}", e);
    //                         };
    //                         sending_message.clear();
    //                     }
    //                     sending_message.push_str(&message);
    //                     sending_message.push('\n');
    //                 }
    //                 if let Err(e) = send_message(
    //                     &channel_id,
    //                     &discord_client_cache_and_http.http,
    //                     &sending_message,
    //                 )
    //                 .await
    //                 {
    //                     eprintln!("Couldn't send message to Discord: {:?}", e);
    //                 };
    //             }
    //         }
    //     });

    //     tokio::spawn(async move {
    //         if let Err(why) = discord_client.start().await {
    //             println!("Discord client error: {:?}", why);
    //         };
    //     });

    let token = env::var("DISCORD_TOKEN").expect("Expected DISCORD_TOKEN in env");

    let channel_id: u64 = env::var("DISCORD_CHANNEL_ID").unwrap().parse().unwrap();

    loop {
        let error = ClientBuilder::new()
            .add_plugin(AvoidKickPlugin)
            .add_plugin(DiscordPlugin {
                token: token.clone(),
                intents: Intents::GUILD_MESSAGES
                    | Intents::MESSAGE_CONTENT
                    | Intents::GUILD_MEMBERS,
            })
            .add_plugin(DiscordBridgePlugin { channel_id })
            .set_handler(mc_handle)
            .start(
                account.clone(),
                env::var("SERVER_IP")
                    .expect("Expected SERVER_IP in env")
                    .as_str(),
            )
            .await;
        eprintln!("{error:?}");
        sleep(Duration::from_secs(4)).await;
    }
}

async fn mc_handle(bot: azalea::Client, event: azalea::Event, _state: State) -> anyhow::Result<()> {
    match event {
        azalea::Event::Login => {}
        azalea::Event::Death(_) => {
            bot.write_packet(ServerboundClientCommandPacket {
                action: azalea_protocol::packets::game::serverbound_client_command_packet::Action::PerformRespawn,
            }.get());
        }
        _ => {}
    }

    Ok(())
}
