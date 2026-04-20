use std::{collections::HashMap, net::SocketAddrV4};

use serde::{Deserialize, Serialize};

use crate::conn::TcpId;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LobbyClient {
    pub udp_addr: Option<SocketAddrV4>,
    pub id: TcpId,
}

impl LobbyClient {
    pub fn new(id: TcpId, udp_addr: Option<SocketAddrV4>) -> Self {
        Self { id, udp_addr }
    }
}

#[derive(Debug)]
pub struct Lobbies {
    // TODO: remove pub
    pub map: HashMap<String, Lobby>,
    pub tcp_id_to_lobby_id: HashMap<TcpId, String>,
}

impl Lobbies {
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
            tcp_id_to_lobby_id: HashMap::new(),
        }
    }

    pub fn cleanup(&mut self, tcp_id: &TcpId) {
        let Some(lobby_id) = self.tcp_id_to_lobby_id.remove(tcp_id) else {
            return;
        };
        let Some(lobby) = self.map.get_mut(&lobby_id) else {
            return;
        };
        lobby.remove_client(*tcp_id);
        if lobby.clients.len() == 0 {
            self.map.remove(&lobby_id);
        }
    }

    pub fn set_client_udp_address(&mut self, id: TcpId, address: SocketAddrV4) -> Option<String> {
        let lobby_id = self.tcp_id_to_lobby_id.get(&id)?;
        let lobby = self.map.get_mut(lobby_id)?;
        lobby.set_udp_address(id, address);
        Some(lobby_id.clone())
    }

    pub fn join(&mut self, id: String, client: LobbyClient) {
        let mut new_lobby = Lobby::new(id.clone());
        new_lobby.add_client(client.clone());

        self.tcp_id_to_lobby_id.insert(client.id, id.clone());
        self.map
            .entry(id)
            .and_modify(|v| v.add_client(client))
            .or_insert(new_lobby);
    }
}

#[derive(Debug)]
pub struct Lobby {
    pub id: String,
    pub clients: Vec<LobbyClient>,
}

impl Lobby {
    fn new(id: String) -> Self {
        Self {
            id,
            clients: vec![],
        }
    }

    fn set_udp_address(&mut self, id: TcpId, address: SocketAddrV4) {
        for client in self.clients.iter_mut() {
            if client.id == id {
                client.udp_addr = Some(address);
                break;
            }
        }
    }

    fn add_client(&mut self, client: LobbyClient) {
        self.remove_client(client.id);
        self.clients.push(client);
    }

    fn remove_client(&mut self, id: TcpId) {
        if let Some((index, _)) = self.clients.iter().enumerate().find(|v| v.1.id == id) {
            self.clients.swap_remove(index);
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LobbyInfoData {
    pub clients: Vec<LobbyClient>,
}

impl LobbyInfoData {
    pub fn new(clients: Vec<LobbyClient>) -> Self {
        Self { clients }
    }

    pub fn from_lobby(lobby: &Lobby) -> Self {
        Self::new(lobby.clients.clone())
    }

    pub fn excluding_client(mut self, id: TcpId) -> Self {
        let index = self.clients.iter().enumerate().find(|v| v.1.id == id);
        if let Some((index, _)) = index {
            self.clients.swap_remove(index);
        }
        self
    }
}
