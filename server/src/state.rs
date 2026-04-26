use std::{collections::HashMap, net::SocketAddrV4};

use serde::{Deserialize, Serialize};

use crate::conn::TcpId;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LobbyClient {
    pub udp_addr: Option<SocketAddrV4>,
    pub is_streaming: bool,
    pub id: TcpId,
}

impl LobbyClient {
    pub fn new(id: TcpId, udp_addr: Option<SocketAddrV4>, is_streaming: bool) -> Self {
        Self {
            id,
            udp_addr,
            is_streaming,
        }
    }
}

#[derive(Debug)]
pub struct Lobbies {
    map: HashMap<String, Lobby>,
    tcp_id_to_lobby_id: HashMap<TcpId, String>,
}

impl Lobbies {
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
            tcp_id_to_lobby_id: HashMap::new(),
        }
    }

    pub fn get(&self, id: &str) -> Option<&Lobby> {
        self.map.get(id)
    }

    pub fn get_tcp_id_lobby(&self, tcp_id: &TcpId) -> Option<&Lobby> {
        let lobby_id = self.tcp_id_to_lobby_id.get(tcp_id)?;
        let lobby = self.map.get(lobby_id)?;
        Some(lobby)
    }

    fn get_tcp_id_lobby_mut(&mut self, tcp_id: &TcpId) -> Option<&mut Lobby> {
        let lobby_id = self.tcp_id_to_lobby_id.get(tcp_id)?;
        let lobby = self.map.get_mut(lobby_id)?;
        Some(lobby)
    }

    pub fn set_client_udp_address(&mut self, id: TcpId, address: SocketAddrV4) -> Option<String> {
        let lobby = self.get_tcp_id_lobby_mut(&id)?;
        lobby.set_udp_address(id, address);
        Some(lobby.id.clone())
    }

    pub fn set_client_is_streaming(&mut self, id: &TcpId, is_streaming: bool) -> Option<String> {
        let lobby = self.get_tcp_id_lobby_mut(id)?;
        lobby.set_is_streaming(id, is_streaming);
        Some(lobby.id.clone())
    }

    pub fn cleanup(&mut self, tcp_id: &TcpId) {
        let Some(lobby) = self.get_tcp_id_lobby_mut(tcp_id) else {
            return;
        };
        lobby.remove_client(*tcp_id);
        if lobby.clients.len() == 0 {
            let lobby_id = lobby.id.clone();
            self.map.remove(&lobby_id);
        }
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

    fn set_udp_address(&mut self, id: TcpId, address: SocketAddrV4) -> Option<()> {
        let client = self.get_tcp_id_client_mut(&id)?;
        client.udp_addr = Some(address);
        Some(())
    }

    fn set_is_streaming(&mut self, id: &TcpId, is_streaming: bool) -> Option<()> {
        let client = self.get_tcp_id_client_mut(id)?;
        client.is_streaming = is_streaming;
        Some(())
    }

    fn get_tcp_id_client_mut(&mut self, id: &TcpId) -> Option<&mut LobbyClient> {
        for client in self.clients.iter_mut() {
            if &client.id == id {
                return Some(client);
            }
        }
        None
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
