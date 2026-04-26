use std::{cell::RefCell, collections::HashMap, io, net::SocketAddr, time};

use futures::{channel::mpsc, prelude::*};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use smol::{
    io::{BufReader, BufWriter},
    net::{TcpStream, UdpSocket},
};

use crate::{conn::TcpId, state};

#[derive(Debug)]
pub struct RpcConn<T: AsyncRead + AsyncWrite> {
    writer: BufWriter<T>,
    reader: BufReader<T>,
}

impl From<TcpStream> for RpcConn<TcpStream> {
    fn from(value: TcpStream) -> Self {
        Self::new(value)
    }
}

impl<T: AsyncRead + AsyncWrite + Clone> RpcConn<T> {
    pub fn new(reader_writer: T) -> Self {
        Self {
            writer: BufWriter::new(reader_writer.clone()),
            reader: BufReader::new(reader_writer),
        }
    }
}

impl<T: AsyncRead + AsyncWrite + Unpin> RpcConn<T> {
    pub fn into_inner(self) -> T {
        self.writer.into_inner()
    }

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

    async fn recv_call_ret<S: Serialize>(&mut self, data: S) -> anyhow::Result<()> {
        Ok(self
            .send_data(serde_json::to_string(&data)?.as_bytes())
            .await?)
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

#[repr(u32)]
pub enum RpcCode {
    Unknown = 0,
    JoinLobby = 1,
    StartStream = 2,
}

impl From<u32> for RpcCode {
    fn from(value: u32) -> Self {
        match value {
            1 => Self::JoinLobby,
            2 => Self::StartStream,
            _ => Self::Unknown,
        }
    }
}

#[repr(u32)]
pub enum RpcNotifyCode {
    Unknown = 0,
    LobbyInfo = 1,
}

impl From<u32> for RpcNotifyCode {
    fn from(value: u32) -> Self {
        match value {
            1 => Self::LobbyInfo,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoidRet {}

#[derive(Debug, Clone)]
pub struct RpcServer {
    lobbies: crate::ArcMu<state::Lobbies>,
    notify_tx: mpsc::Sender<Notify>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct JoinLobbyData {
    pub id: String,
}

#[derive(Debug)]
pub struct RpcUserClient {
    connection: RpcConn<TcpStream>,
}

impl RpcUserClient {
    pub fn new(connection: RpcConn<TcpStream>) -> Self {
        Self { connection }
    }

    pub async fn join_lobby(&mut self, data: JoinLobbyData) -> anyhow::Result<VoidRet> {
        Ok(self
            .connection
            .call(RpcCode::JoinLobby as u32, data)
            .await?)
    }

    pub async fn start_stream(&mut self) -> anyhow::Result<VoidRet> {
        Ok(self
            .connection
            .call(RpcCode::StartStream as u32, VoidRet {})
            .await?)
    }
}

pub fn rpc_user_notify_stream(
    connection: RpcConn<TcpStream>,
) -> impl TryStream<Item = anyhow::Result<state::LobbyInfoData>> {
    futures::stream::try_unfold(connection, |mut v| async {
        let (code, data) = v.recv_call::<RpcNotifyCode>().await?;
        match code {
            RpcNotifyCode::Unknown => anyhow::bail!("unknown code"),
            RpcNotifyCode::LobbyInfo => {
                v.recv_call_ret(VoidRet {}).await?;
                Ok(Some((
                    serde_json::from_slice::<state::LobbyInfoData>(&data)?,
                    v,
                )))
            }
        }
    })
}

#[derive(Debug)]
pub struct RpcServerHandler {
    id: TcpId,
    server: RpcServer,
    connection: RpcConn<TcpStream>,
}

impl RpcServerHandler {
    async fn handle_join_lobby(&self, data: JoinLobbyData) -> anyhow::Result<VoidRet> {
        self.server.lobbies.lock().await.join(
            data.id.clone(),
            state::LobbyClient::new(self.id, None, false),
        );
        self.server.notify_lobby(&data.id).await?;
        Ok(VoidRet {})
    }

    async fn handle_start_stream(&self, _data: VoidRet) -> anyhow::Result<VoidRet> {
        let lobby_id = self
            .server
            .lobbies
            .lock()
            .await
            .set_client_is_streaming(&self.id, true)
            .ok_or_else(|| anyhow::anyhow!("start stream no lobby"))?;
        self.server.notify_lobby(&lobby_id).await?;
        Ok(VoidRet {})
    }

    pub async fn listen(mut self) -> anyhow::Result<()> {
        println!("handler.listen");
        loop {
            let (code, data) = self.connection.recv_call::<RpcCode>().await?;
            match code {
                RpcCode::Unknown => anyhow::bail!("unknown code"),
                RpcCode::JoinLobby => {
                    let ret = self
                        .handle_join_lobby(serde_json::from_slice(&data)?)
                        .await?;
                    self.connection.recv_call_ret(ret).await?;
                }
                RpcCode::StartStream => {
                    let ret = self
                        .handle_start_stream(serde_json::from_slice(&data)?)
                        .await?;
                    self.connection.recv_call_ret(ret).await?;
                }
            };
        }
    }
}

impl Drop for RpcServerHandler {
    fn drop(&mut self) {
        let server = self.server.clone();
        let id = self.id.clone();
        smol::spawn(async move {
            match server.cleanup(&id).await {
                Ok(_) => {}
                Err(v) => {
                    println!("cleaning up server failed: {v}");
                }
            };
        })
        .detach();
    }
}

impl RpcServer {
    pub fn new(notify_tx: mpsc::Sender<Notify>) -> Self {
        Self {
            lobbies: crate::arcmu(state::Lobbies::new()),
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
            println!("got udp message: {buf:?}, addr: {addr}");
            let lobby_id = self.lobbies.lock().await.set_client_udp_address(buf, v4);
            if let Some(lobby_id) = lobby_id {
                self.notify_lobby(&lobby_id).await?;
            } else {
                println!("warn: udp connection init without lobby");
            }
        }
    }

    pub async fn listen(&self) -> anyhow::Result<()> {
        futures::try_join!(self.listen_for_udp_addresses())?;
        Ok(())
    }

    async fn cleanup(&self, id: &TcpId) -> anyhow::Result<()> {
        self.cleanup_lobbies(id).await?;
        Ok(())
    }

    async fn cleanup_lobbies(&self, id: &TcpId) -> anyhow::Result<()> {
        let lobby_id = {
            let mut lobbies = self.lobbies.lock().await;
            let Some(lobby_id) = lobbies.get_tcp_id_lobby(id).map(|v| v.id.clone()) else {
                return Ok(());
            };
            lobbies.cleanup(&id);
            lobby_id
        };
        self.notify_lobby(&lobby_id).await?;
        Ok(())
    }

    async fn notify_lobby(&self, lobby_id: &str) -> anyhow::Result<()> {
        println!("notifying lobby \"{lobby_id}\"");

        let lobbies = self.lobbies.lock().await;
        let Some(lobby) = lobbies.get(lobby_id) else {
            return Ok(());
        };

        let notify_lobby = lobby
            .clients
            .iter()
            .map(|client| NotifyLobby {
                tcp_id: client.id,
                lobby_info: state::LobbyInfoData::from_lobby(lobby).excluding_client(client.id),
            })
            .collect::<Vec<_>>();

        self.notify_tx
            .clone()
            .send(Notify::Lobby(notify_lobby))
            .await?;
        Ok(())
    }

    pub fn get_handler(&self, id: TcpId, connection: RpcConn<TcpStream>) -> RpcServerHandler {
        RpcServerHandler {
            server: self.clone(),
            id,
            connection,
        }
    }
}

#[derive(Debug)]
pub struct RpcNotifyClient {
    connection: RpcConn<TcpStream>,
}

impl RpcNotifyClient {
    pub fn new(connection: RpcConn<TcpStream>) -> Self {
        Self { connection }
    }

    async fn send_lobby_info(&mut self, data: state::LobbyInfoData) -> anyhow::Result<VoidRet> {
        Ok(self
            .connection
            .call(RpcNotifyCode::LobbyInfo as u32, data)
            .await?)
    }
}

#[derive(Debug)]
pub struct NotifyLobby {
    tcp_id: TcpId,
    lobby_info: state::LobbyInfoData,
}

#[derive(Debug)]
pub enum Notify {
    Lobby(Vec<NotifyLobby>),
    NewReceiver(TcpId, RpcNotifyClient),
}

pub struct Notifier {
    receivers: HashMap<TcpId, RefCell<Option<RpcNotifyClient>>>,
    notify_rx: mpsc::Receiver<Notify>,
}

impl Notifier {
    pub fn new(notify_rx: mpsc::Receiver<Notify>) -> Self {
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

    pub async fn listen(mut self) -> anyhow::Result<()> {
        let new_cleanup_timer =
            || futures::FutureExt::fuse(smol::Timer::interval(time::Duration::from_secs(1)));
        let mut cleanup_timer = new_cleanup_timer();
        loop {
            futures::select_biased! {
                notify_res = self.notify_rx.recv() => match notify_res? {
                    Notify::Lobby(v) => self.notify_lobby(v).await?,
                    Notify::NewReceiver(id, v) => {
                        self.receivers.insert(id, RefCell::new(Some(v)));
                    }
                },
                _ = cleanup_timer => {
                    cleanup_timer = new_cleanup_timer();
                    self.cleanup();
                }
            }
        }
    }
}
