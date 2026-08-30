use crate::session::{Connection, Server};
use anyhow::Result;
use tera_protocol::value::Object;

pub struct Reason {
    pub packet: &'static str,
    pub why: &'static str,
}

pub const DELIBERATE: [Reason; 22] = [
    Reason { packet: "C_RQ_SKILL_POLISHING_EXP_INFO", why: "no skill polishing subsystem, and the reply would have to state an experience total we do not track" },
    Reason { packet: "C_SET_SERVANT_SEQUENCE", why: "no servant subsystem; acknowledging an order we cannot keep would desync the client's slots" },
    Reason { packet: "C_REQUEST_SERVANT_ADVENTURE_LIST", why: "no servant subsystem, and the reply packet cannot be encoded from our definitions" },
    Reason { packet: "C_NOTIFY_LOCATION_IN_ACTION", why: "a position notice during an action; the client does not wait for an answer" },
    Reason { packet: "C_TEL_CAMP", why: "no campfire teleport network; answering would offer destinations that do not exist" },
    Reason { packet: "C_NPC_CONTACT", why: "no dialogue, shop or quest system behind any npc yet" },
    Reason { packet: "C_REQUEST_GUILD_PERK_LIST", why: "the player has no guild; a default-filled perk list crashed the client once already" },
    Reason { packet: "C_HARDWARE_INFO", why: "telemetry the client volunteers; nothing is expected back" },
    Reason { packet: "C_CHANGE_USER_LOBBY_SLOT_ID", why: "cosmetic lobby ordering we do not persist" },
    Reason { packet: "C_UPDATE_CONTENTS_PLAYTIME", why: "playtime telemetry the client volunteers" },
    Reason { packet: "C_REQUEST_INGAMESTORE_PRODUCT_LIST", why: "no store; an empty catalogue still asserts the store exists and is open" },
    Reason { packet: "C_TRADE_BROKER_HIGHEST_ITEM_LEVEL", why: "no broker; any figure we sent would be invented" },
    Reason { packet: "C_COLLECTION_PICKSTART", why: "no gathering system; announcing a pick duration starts a bar that never finishes" },
    Reason { packet: "C_DELETE_PARCEL", why: "no mail system; ok=true makes the client drop a parcel that still exists on our side" },
    Reason { packet: "C_GET_GUILD_WARE_HISTORY", why: "the player has no guild, so a page of bank history would be invented" },
    Reason { packet: "C_JOIN_PRIVATE_CHANNEL", why: "no private channel registry; handing out channel 1 promises a channel we never route" },
    Reason { packet: "C_REQUEST_ACCESSORY_COST_INFO", why: "no accessory upgrade costs anywhere in our catalogues" },
    Reason { packet: "C_REQUEST_GUILD_LIST", why: "no guilds exist; claiming one page of results would show an empty list as if it were the truth" },
    Reason { packet: "C_VIEW_BATTLE_FIELD_RESULT", why: "no battleground history; the seven day window would be fabricated" },
    Reason { packet: "C_WHISPER", why: "whispers have no delivery path yet; echoing one back would look delivered" },
    Reason { packet: "C_DIALOG_EVENT", why: "no npc dialogue tree; acknowledging a choice would advance a conversation that does not exist" },
    Reason { packet: "C_SELECT_CHANNEL", why: "this world runs a single channel; confirming a switch would promise an instance we never create" },
];

pub fn owns(name: &str) -> bool {
    DELIBERATE.iter().any(|entry| entry.packet == name)
}

pub fn reason(name: &str) -> Option<&'static str> {
    DELIBERATE
        .iter()
        .find(|entry| entry.packet == name)
        .map(|entry| entry.why)
}

pub fn handle(
    name: &str,
    _request: Option<&Object>,
    server: &Server<'_>,
    connection: &mut Connection,
) -> Result<()> {
    if connection.announced.insert(name.to_string()) {
        if let Some(why) = reason(name) {
            server.logger.line(format!("   answering nothing on purpose: {why}"));
        }
    }
    Ok(())
}
