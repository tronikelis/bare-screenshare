use std::{
    cell::RefCell,
    collections::HashMap,
    fs,
    io::{self, prelude::*},
    net::{SocketAddr, SocketAddrV4},
    sync::Arc,
    time,
};

type ArcMu<T> = Arc<Mutex<T>>;

fn arcmu<T>(inner: T) -> ArcMu<T> {
    Arc::new(Mutex::new(inner))
}

use futures::{channel::mpsc, prelude::*};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use smol::{
    io::{BufReader, BufWriter},
    lock::Mutex,
    net::{TcpListener, TcpStream, UdpSocket},
};

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
    async fn call_raw<Code: Into<u32>>(&mut self, code: Code, data: &[u8]) -> io::Result<Vec<u8>> {
        self.send_code(code).await?;
        self.send_data(data).await?;
        self.recv_data().await
    }

    async fn call<Code: Into<u32>, S: Serialize, D: DeserializeOwned>(
        &mut self,
        code: Code,
        data: S,
    ) -> anyhow::Result<D> {
        let ret = self
            .call_raw(code, serde_json::to_string(&data)?.as_bytes())
            .await?;
        Ok(serde_json::from_slice(&ret)?)
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
    id_map: ArcMu<HashMap<TcpId, TcpStream>>,
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
            id_map: arcmu(HashMap::new()),
        })
    }

    async fn listen(self) -> anyhow::Result<()> {
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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

impl From<&Lobby> for LobbyInfo {
    fn from(value: &Lobby) -> Self {
        Self {
            clients: value.clients.clone(),
        }
    }
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
enum RpcNotifyCode {
    Unknown = 0,
    LobbyInfo = 1,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LobbyInfo {
    clients: Vec<LobbyClient>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct VoidRet {}

impl LobbyInfo {
    fn excluding_client(mut self, id: TcpId) -> Self {
        let index = self.clients.iter().enumerate().find(|v| v.1.id == id);
        if let Some((index, _)) = index {
            self.clients.swap_remove(index);
        }
        self
    }
}

impl From<u32> for RpcNotifyCode {
    fn from(value: u32) -> Self {
        match value {
            1 => Self::LobbyInfo,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug)]
struct RpcNotifyClient {
    connection: RpcConn<TcpStream>,
}

impl RpcNotifyClient {
    fn new(connection: RpcConn<TcpStream>) -> Self {
        Self { connection }
    }

    async fn send_lobby_info(&mut self, data: LobbyInfo) -> anyhow::Result<VoidRet> {
        Ok(self
            .connection
            .call(RpcNotifyCode::LobbyInfo as u32, data)
            .await?)
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

#[derive(Debug)]
struct NotifyLobby {
    tcp_id: TcpId,
    lobby_info: LobbyInfo,
}

#[derive(Debug)]
enum Notify {
    Lobby(Vec<NotifyLobby>),
    NewReceiver(TcpId, RpcNotifyClient),
}

#[derive(Debug, Clone)]
struct RpcServer {
    lobbies: ArcMu<Lobbies>,
    notify_tx: mpsc::Sender<Notify>,
}

#[derive(Debug)]
struct RpcServerHandler {
    id: TcpId,
    server: RpcServer,
    connection: RpcConn<TcpStream>,
}

impl RpcServerHandler {
    async fn handle_join_lobby(&mut self, data: JoinLobbyData) -> anyhow::Result<VoidRet> {
        self.server
            .lobbies
            .lock()
            .await
            .join(data.id.clone(), LobbyClient::new(self.id, None));
        self.server.notify_lobby(&data.id).await?;
        Ok(VoidRet {})
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
    fn new(notify_tx: mpsc::Sender<Notify>) -> Self {
        Self {
            lobbies: arcmu(Lobbies::new()),
            notify_tx,
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
        futures::try_join!(self.listen_for_udp_addresses())?;
        Ok(())
    }

    async fn cleanup(&self, id: TcpId) {
        self.lobbies.lock().await.cleanup(id);
    }

    async fn notify_lobby(&mut self, lobby_id: &str) -> anyhow::Result<()> {
        let lobbies = self.lobbies.lock().await;
        let Some(lobby) = lobbies.map.get(lobby_id) else {
            return Ok(());
        };

        let notify_lobby = lobby
            .clients
            .iter()
            .map(|client| NotifyLobby {
                tcp_id: client.id,
                lobby_info: LobbyInfo::from(lobby).excluding_client(client.id),
            })
            .collect::<Vec<_>>();

        self.notify_tx.send(Notify::Lobby(notify_lobby)).await?;
        Ok(())
    }

    fn get_handler(&self, id: TcpId, connection: RpcConn<TcpStream>) -> RpcServerHandler {
        RpcServerHandler {
            server: self.clone(),
            id,
            connection,
        }
    }
}

struct Notifier {
    receivers: HashMap<TcpId, RefCell<Option<RpcNotifyClient>>>,
    notify_rx: mpsc::Receiver<Notify>,
}

impl Notifier {
    fn new(notify_rx: mpsc::Receiver<Notify>) -> Self {
        Self {
            receivers: HashMap::new(),
            notify_rx,
        }
    }

    async fn notify_lobby(&self, lobbies: Vec<NotifyLobby>) -> anyhow::Result<()> {
        let futures = lobbies
            .iter()
            .map(|v| async {
                let Some(receiver) = self.receivers.get(&v.tcp_id) else {
                    return Result::<(), anyhow::Error>::Ok(());
                };
                let mut should_remove = false;
                if let Some(receiver) = receiver.borrow_mut().as_mut() {
                    should_remove = receiver
                        .send_lobby_info(v.lobby_info.clone())
                        .await
                        .is_err();
                }
                if should_remove {
                    *receiver.borrow_mut() = None;
                }
                Ok(())
            })
            .collect::<Vec<_>>();

        for fut in futures::future::join_all(futures).await {
            fut?;
        }
        Ok(())
    }

    fn cleanup(&mut self) {
        self.receivers.retain(|_, v| v.borrow().is_some());
    }

    async fn listen(mut self) -> anyhow::Result<()> {
        let mut cleanup_timer =
            futures::FutureExt::fuse(smol::Timer::interval(time::Duration::from_secs(1)));
        loop {
            futures::select_biased! {
                notify_res = self.notify_rx.recv() => match notify_res? {
                    Notify::Lobby(v) => self.notify_lobby(v).await?,
                    Notify::NewReceiver(id, v) => {
                        self.receivers.insert(id, RefCell::new(Some(v)));
                    }
                },
                _ = cleanup_timer => {
                    cleanup_timer = futures::FutureExt::fuse(smol::Timer::interval(time::Duration::from_secs(1)));
                    self.cleanup();
                }
            }
        }
    }
}

async fn async_main() -> anyhow::Result<()> {
    let (mut notify_tx, notify_rx) = mpsc::channel(8);

    let rpc_server = RpcServer::new(notify_tx.clone());
    let notifier = Notifier::new(notify_rx);

    let (accept_tx, mut accept_rx) = mpsc::channel(8);
    let tcp_send_receive = TcpSendReceive::new("127.0.0.1", 3000, accept_tx).await?;

    let handler_fut = async {
        loop {
            let (tcp_id, sender, receiver) = match accept_rx.recv().await {
                Ok(v) => v,
                Err(v) => anyhow::bail!(v),
            };

            notify_tx
                .send(Notify::NewReceiver(
                    tcp_id,
                    RpcNotifyClient::new(receiver.into()),
                ))
                .await?;
            let handler = rpc_server.get_handler(tcp_id, sender.into());

            smol::spawn(async move {
                let _ = handler.listen().await;
            })
            .detach();
        }
    };

    println!("started listening");
    let (_, _, _, _): ((), (), (), ()) = futures::try_join!(
        tcp_send_receive.listen(),
        rpc_server.listen(),
        notifier.listen(),
        handler_fut,
    )?;

    Ok(())
}

fn main() {
    smol::block_on(async_main()).unwrap();
}
