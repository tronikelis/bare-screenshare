use std::{
    collections::HashMap,
    fs,
    io::{self, prelude::*},
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    sync::Arc,
};

use futures::{channel::mpsc, prelude::*};
use serde::{Deserialize, Serialize};
use smol::{
    io::{BufReader, BufWriter},
    lock::Mutex,
    net::{TcpListener, TcpStream, UdpSocket},
};

mod macros;

#[derive(Debug)]
struct RpcConn<T: AsyncRead + AsyncWrite> {
    writer: BufWriter<T>,
    reader: BufReader<T>,
}

impl From<TcpStream> for RpcConn<TcpStream> {
    fn from(value: TcpStream) -> Self {
        RpcConn {
            writer: BufWriter::new(value.clone()),
            reader: BufReader::new(value),
        }
    }
}

impl<T: AsyncRead + AsyncWrite + Unpin> RpcConn<T> {
    async fn call<Code: Into<u32>>(&mut self, code: Code, data: &[u8]) -> io::Result<Vec<u8>> {
        self.send_code(code).await?;
        self.send_data(data).await?;
        self.recv_data().await
    }

    async fn recv_call<Code: From<u32>>(&mut self) -> io::Result<(Code, Vec<u8>)> {
        let code = self.recv_code::<Code>().await?;
        let data = self.recv_data().await?;
        Ok((code, data))
    }

    async fn recv_call_ret(&mut self, data: &[u8]) -> io::Result<()> {
        self.send_data(data).await
    }

    async fn recv_data(&mut self) -> io::Result<Vec<u8>> {
        let len = self.read_len().await?;
        let mut data = Vec::with_capacity(len as _);
        unsafe { data.set_len(len as _) }
        self.reader.read_exact(&mut data).await?;

        Ok(data)
    }

    async fn read_len(&mut self) -> io::Result<u32> {
        let mut len = [0; 4];
        self.reader.read_exact(&mut len).await?;
        Ok(u32::from_le_bytes(len))
    }

    async fn write_len(&mut self, len: u32) -> io::Result<()> {
        self.writer.write_all(&len.to_le_bytes()).await
    }

    async fn recv_data_exact(&mut self, buf: &mut [u8]) -> io::Result<()> {
        let len = self.read_len().await?;
        if len != buf.len() as u32 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "recv_exact incorrect len",
            ));
        }
        self.reader.read_exact(buf).await
    }

    async fn send_code<Code: Into<u32>>(&mut self, code: Code) -> io::Result<()> {
        self.writer.write_all(&(code.into()).to_le_bytes()).await
    }

    async fn recv_code<Code: From<u32>>(&mut self) -> io::Result<Code> {
        let mut code = [0; 4];
        self.reader.read_exact(&mut code).await?;
        Ok(u32::from_le_bytes(code).into())
    }

    async fn send_data(&mut self, data: &[u8]) -> io::Result<()> {
        self.write_len(data.len() as _).await?;
        self.writer.write_all(data).await?;
        self.writer.flush().await?;
        Ok(())
    }
}

fn rand_bytes(buf: &mut [u8]) -> io::Result<()> {
    let mut dev_random = fs::File::open("/dev/urandom")?;
    dev_random.read_exact(buf)
}

type TcpId = [u8; 32];

type TcpSenderReceiver = (TcpId, TcpStream, TcpStream);

struct TcpSendReceive {
    sub_listener: TcpListener,
    rpc_listener: TcpListener,
    id_map: Arc<Mutex<HashMap<TcpId, TcpStream>>>,
    accept_tx: mpsc::Sender<TcpSenderReceiver>,
}

impl TcpSendReceive {
    async fn new(
        host: &str,
        port: usize,
        accept_tx: mpsc::Sender<TcpSenderReceiver>,
    ) -> anyhow::Result<Self> {
        let sub_listener = TcpListener::bind(format!("{}:{}", host, port)).await?;
        let rpc_listener = TcpListener::bind(format!("{}:{}", host, port + 1)).await?;

        Ok(Self {
            accept_tx,
            sub_listener,
            rpc_listener,
            id_map: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    async fn listen(&self) -> anyhow::Result<()> {
        let sub_listener = async {
            loop {
                let (mut stream, _) = self.sub_listener.accept().await?;
                let mut id: TcpId = [0; 32];
                rand_bytes(&mut id)?;
                stream.write_all(&id).await?;
                self.id_map.lock().await.insert(id, stream);
            }
        };

        let rpc_listener = async {
            loop {
                let (mut stream, _) = self.rpc_listener.accept().await?;
                let id_map = self.id_map.clone();
                let mut accept_tx = self.accept_tx.clone();
                smol::spawn::<anyhow::Result<()>>(async move {
                    let mut id: TcpId = [0; 32];
                    stream.read_exact(&mut id).await?;

                    let mut id_map = id_map.lock().await;
                    if !id_map.contains_key(&id) {
                        return Ok(());
                    }
                    let sender = id_map
                        .remove(&id)
                        .ok_or_else(|| anyhow::anyhow!("expected id to be in map"))?;
                    accept_tx.send((id, sender, stream)).await?;

                    Ok(())
                })
                .detach();
            }
        };

        let (res1, res2): (anyhow::Result<()>, anyhow::Result<()>) =
            futures::join!(sub_listener, rpc_listener);
        res1?;
        res2?;

        Ok(())
    }
}

#[derive(Debug, Clone)]
struct LobbyClient {
    udp_addr: Option<SocketAddrV4>,
    id: TcpId,
}

impl LobbyClient {
    fn new(id: TcpId, udp_addr: Option<SocketAddrV4>) -> Self {
        Self { id, udp_addr }
    }
}

#[derive(Debug)]
struct Lobbies {
    map: HashMap<String, Lobby>,
    tcp_id_to_lobby_id: HashMap<TcpId, String>,
}

impl Lobbies {
    fn new() -> Self {
        Self {
            map: HashMap::new(),
            tcp_id_to_lobby_id: HashMap::new(),
        }
    }

    fn cleanup(&mut self, tcp_id: TcpId) {
        let Some(lobby_id) = self.tcp_id_to_lobby_id.remove(&tcp_id) else {
            return;
        };
        let Some(lobby) = self.map.get_mut(&lobby_id) else {
            return;
        };
        lobby.remove_client(tcp_id);
        if lobby.clients.len() == 0 {
            self.map.remove(&lobby_id);
        }
    }

    fn set_client_udp_address(&mut self, id: TcpId, address: SocketAddrV4) {
        let Some(lobby_id) = self.tcp_id_to_lobby_id.get(&id) else {
            return;
        };
        let Some(lobby) = self.map.get_mut(lobby_id) else {
            return;
        };
        lobby.set_udp_address(id, address);
    }

    fn join(&mut self, id: String, client: LobbyClient) {
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
struct Lobby {
    id: String,
    clients: Vec<LobbyClient>,
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

#[repr(u32)]
enum RpcServerCode {
    Unknown = 0,
    JoinLobby = 1,
}

impl From<u32> for RpcServerCode {
    fn from(value: u32) -> Self {
        match value {
            1 => Self::JoinLobby,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct JoinLobbyData {
    id: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct JoinLobbyRet {}

#[derive(Debug, Clone)]
struct RpcServer {
    receivers: Arc<Mutex<HashMap<TcpId, RpcConn<TcpStream>>>>,
    lobbies: Arc<Mutex<Lobbies>>,
}

#[derive(Debug)]
struct RpcServerHandler {
    id: TcpId,
    server: RpcServer,
    connection: RpcConn<TcpStream>,
}

impl RpcServerHandler {
    async fn handle_join_lobby(&mut self, data: JoinLobbyData) -> anyhow::Result<JoinLobbyRet> {
        self.server
            .lobbies
            .lock()
            .await
            .join(data.id, LobbyClient::new(self.id, None));
        Ok(JoinLobbyRet {})
    }

    async fn listen(mut self) -> anyhow::Result<()> {
        loop {
            let (code, data) = self.connection.recv_call::<RpcServerCode>().await?;
            let ret = match code {
                RpcServerCode::Unknown => anyhow::bail!("unknown code"),
                RpcServerCode::JoinLobby => serde_json::to_string(
                    &self
                        .handle_join_lobby(serde_json::from_slice(&data)?)
                        .await?,
                )?,
            };
            self.connection.recv_call_ret(ret.as_bytes()).await?;
        }
    }
}

impl Drop for RpcServerHandler {
    fn drop(&mut self) {
        let server = self.server.clone();
        let id = self.id.clone();
        smol::spawn(async move {
            server.cleanup(id).await;
        })
        .detach();
    }
}

impl RpcServer {
    fn new() -> Self {
        Self {
            receivers: Arc::new(Mutex::new(HashMap::new())),
            lobbies: Arc::new(Mutex::new(Lobbies::new())),
        }
    }

    async fn listen_for_udp_addresses(&self) -> anyhow::Result<()> {
        let socket = UdpSocket::bind("127.0.0.1:4000").await?;
        loop {
            let mut buf: TcpId = [0; 32];
            let Ok((size, addr)) = socket.recv_from(&mut buf).await else {
                continue;
            };
            if size != 32 {
                println!("udp size not 32");
                continue;
            }
            let SocketAddr::V4(v4) = addr else {
                unreachable!();
            };
            self.lobbies.lock().await.set_client_udp_address(buf, v4);
        }
    }

    async fn listen(&self) -> anyhow::Result<()> {
        let (res1,) = futures::join!(self.listen_for_udp_addresses());
        res1?;
        Ok(())
    }

    async fn cleanup(&self, id: TcpId) {
        self.receivers.lock().await.remove(&id);
        self.lobbies.lock().await.cleanup(id);
    }

    async fn add_receiver(&self, id: TcpId, receiver: RpcConn<TcpStream>) {
        self.receivers.lock().await.insert(id, receiver);
    }

    fn get_handler(&self, id: TcpId, connection: RpcConn<TcpStream>) -> RpcServerHandler {
        RpcServerHandler {
            server: self.clone(),
            id,
            connection,
        }
    }
}

async fn async_main() -> anyhow::Result<()> {
    let rpc_server = RpcServer::new();

    let (accept_tx, mut accept_rx) = mpsc::channel(8);
    let tcp_send_receive = TcpSendReceive::new("127.0.0.1", 3000, accept_tx).await?;

    let receive_fut = async {
        loop {
            let (tcp_id, sender, receiver) = accept_rx.recv().await?;
            rpc_server.add_receiver(tcp_id, receiver.into()).await;
            let handler = rpc_server.get_handler(tcp_id, sender.into());
            smol::spawn(async move {
                let _ = handler.listen().await;
            })
            .detach();
        }
    };

    let (res1, res2, res3): (anyhow::Result<()>, anyhow::Result<()>, anyhow::Result<()>) =
        futures::join!(tcp_send_receive.listen(), rpc_server.listen(), receive_fut,);

    res1?;
    res2?;
    res3?;

    Ok(())
}

fn main() {
    smol::block_on(async_main()).unwrap();
}
