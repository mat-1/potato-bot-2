//! An Azalea plugin that helps you avoid getting kicked for spamming or for
//! sending illegal chat messages.

use azalea::ecs::{component::Component, AppTickExt};
use bevy_app::{App, Plugin};
use bevy_ecs::{
    entity::Entity,
    event::{EventReader, EventWriter},
    system::{Commands, Query},
};

pub struct AvoidKickPlugin;

impl Plugin for AvoidKickPlugin {
    fn build(&self, app: &mut App) {
        app.add_event::<SendChatEvent>()
            .add_system(send_chat_listener)
            .add_tick_system(drain_chat_message_queue);
    }
}

#[derive(Component)]
pub struct AvoidChatKick {
    pub queued_messages: Vec<String>,
    pub chat_spam_tick_count: usize,
}

pub struct SendChatEvent {
    entity: Entity,
    content: String,
}

impl SendChatEvent {
    pub fn new(entity: Entity, content: &str) -> Option<Self> {
        let content = content.to_string();
        if message_legal_to_minecraft(&content) {
            Some(Self { entity, content })
        } else {
            None
        }
    }
}

/// Whether this message can be sent to Minecraft without the server kicking us.
fn message_legal_to_minecraft(message: &str) -> bool {
    if message.len() > 256 {
        return false;
    }
    for char in message.chars() {
        if matches!(char, '\x00'..='\x1F' | '\x7F' | '§') {
            return false;
        }
    }

    true
}

fn send_chat_listener(
    mut commands: Commands,
    mut events: EventReader<SendChatEvent>,
    mut query: Query<Option<&mut AvoidChatKick>>,
) {
    for event in events.iter() {
        let Ok(state) = query.get_mut(event.entity) else {
            continue;
        };

        if let Some(mut state) = state {
            state.queued_messages.push(event.content.clone());
        } else {
            commands.entity(event.entity).insert(AvoidChatKick {
                queued_messages: vec![event.content.clone()],
                chat_spam_tick_count: 0,
            });
        }
    }
}

fn drain_chat_message_queue(
    mut query: Query<(Entity, &mut AvoidChatKick)>,
    mut chat_message_events: EventWriter<azalea::chat::SendChatEvent>,
) {
    for (entity, mut state) in query.iter_mut() {
        // decrease the chat_spam_tick_count every tick (unless it's 0)
        if state.chat_spam_tick_count > 0 {
            state.chat_spam_tick_count -= 1;
        }

        // the 100 is actually 200 in vanilla, but i chose 100 to make sure it doesn't go over
        let max_drain = (100 - state.chat_spam_tick_count) / 20;
        let len = state.queued_messages.len();
        let len = max_drain.min(len);
        state.chat_spam_tick_count += len * 20;

        for message in state.queued_messages.drain(..len) {
            println!("draining chat message: {message}");
            chat_message_events.send(azalea::chat::SendChatEvent {
                entity,
                content: message.clone(),
            });
        }
    }
}
