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
    async fn new(host: &str, port: usize, accept_tx: mpsc::Sender<TcpSenderReceiver>) -> Self {
        let sub_listener = TcpListener::bind(format!("{}:{}", host, port))
            .await
            .unwrap();
        let rpc_listener = TcpListener::bind(format!("{}:{}", host, port + 1))
            .await
            .unwrap();

        Self {
            accept_tx,
            sub_listener,
            rpc_listener,
            id_map: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    async fn listen(&self) {
        let sub_listener = async {
            loop {
                let (mut stream, _) = self.sub_listener.accept().await.unwrap();
                let mut id: TcpId = [0; 32];
                rand_bytes(&mut id).unwrap();
                stream.write_all(&id).await.unwrap();
                self.id_map.lock().await.insert(id, stream);
            }
        };

        let rpc_listener = async {
            loop {
                let (mut stream, _) = self.rpc_listener.accept().await.unwrap();
                let id_map = self.id_map.clone();
                let mut accept_tx = self.accept_tx.clone();
                smol::spawn(async move {
                    let mut id: TcpId = [0; 32];
                    stream.read_exact(&mut id).await.unwrap();

                    let mut id_map = id_map.lock().await;
                    if !id_map.contains_key(&id) {
                        return;
                    }
                    let sender = id_map.remove(&id).expect("map contained the key");

                    accept_tx.send((id, sender, stream)).await.unwrap();
                })
                .detach();
            }
        };

        futures::join!(sub_listener, rpc_listener);
    }
}

#[repr(u32)]
enum RpcCode {
    Unknown = 0,
    JoinLobby = 1,
    ReceiveLobbyClients = 2,
}

impl From<u32> for RpcCode {
    fn from(value: u32) -> Self {
        match value {
            1 => Self::JoinLobby,
            2 => Self::ReceiveLobbyClients,
            _ => Self::Unknown,
        }
    }
}

#[derive(Serialize, Deserialize)]
struct LobbyClientPayload {
    udp_octets: [u8; 4],
    udp_port: u16,
    id: TcpId,
}

impl LobbyClientPayload {
    fn new(id: TcpId, addr: SocketAddrV4) -> Self {
        Self {
            id,
            udp_octets: addr.ip().octets(),
            udp_port: addr.port(),
        }
    }
}

struct LobbyClient {
    udp_addr: SocketAddrV4,
    receiver: RpcConn<TcpStream>,
    id: TcpId,
}

impl LobbyClient {
    fn new(id: TcpId, udp_addr: SocketAddrV4, receiver: RpcConn<TcpStream>) -> Self {
        Self {
            id,
            udp_addr,
            receiver,
        }
    }
}

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

    fn add_client(&mut self, client: LobbyClient) {
        self.remove_client(client.id);
        self.clients.push(client);
    }

    fn remove_client(&mut self, id: TcpId) {
        if let Some((index, _)) = self.clients.iter().enumerate().find(|v| v.1.id == id) {
            self.clients.swap_remove(index);
        }
    }

    fn new_with_client(id: String, client: LobbyClient) -> Self {
        let mut c = Self::new(id);
        c.add_client(client);
        c
    }

    async fn notify_lobby_clients(&mut self) -> io::Result<()> {
        let peers = self
            .clients
            .iter()
            .map(|v| (v.id, v.udp_addr))
            .collect::<Vec<_>>();

        let futures = self
            .clients
            .iter_mut()
            .map(|client| async {
                let payload = peers
                    .iter()
                    .filter(|v| v.0 != client.id)
                    .map(|v| LobbyClientPayload::new(v.0, v.1))
                    .collect::<Vec<_>>();
                client
                    .receiver
                    .call(
                        RpcCode::ReceiveLobbyClients as u32,
                        serde_json::to_string(&payload)
                            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?
                            .as_bytes(),
                    )
                    .await
            })
            .collect::<Vec<_>>();

        for result in futures::future::join_all(futures).await {
            result?;
        }

        Ok(())
    }
}

fn main() {
    smol::block_on(async {
        let udp_addresses = Arc::new(Mutex::new(HashMap::<TcpId, SocketAddrV4>::new()));

        let udp_listener = async {
            let socket = UdpSocket::bind("127.0.0.1:4000").await.unwrap();
            loop {
                let mut buf: TcpId = [0; 32];
                let (size, addr) = socket.recv_from(&mut buf).await.unwrap();
                if size != 32 {
                    println!("udp size not 32");
                    continue;
                }
                let SocketAddr::V4(v4) = addr else {
                    unreachable!();
                };
                udp_addresses.lock().await.insert(buf, v4);
            }
        };

        let (accept_tx, mut accept_rx) = mpsc::channel(1);
        let tcp_send_receive = TcpSendReceive::new("localhost", 3000, accept_tx).await;

        let lobbies = Arc::new(Mutex::new(HashMap::<String, Lobby>::new()));

        println!("started server");
        futures::join!(udp_listener, tcp_send_receive.listen(), async {
            loop {
                let (id, sender, receiver) = accept_rx.recv().await.unwrap();
                let mut sender: RpcConn<_> = sender.into();
                let mut receiver: Option<RpcConn<_>> = Some(receiver.into());
                println!("got connection");

                let lobbies = lobbies.clone();
                let udp_addresses = udp_addresses.clone();
                smol::spawn(async move {
                    loop {
                        let (code, data) = sender.recv_call::<RpcCode>().await.unwrap();
                        match code {
                            RpcCode::JoinLobby => {
                                let udp_address =
                                    udp_addresses.lock().await.get(&id).map(|v| v.clone());
                                let Some(udp_addr) = udp_address else {
                                    panic!("joining lobby without establishing udp, todo fix");
                                };

                                let lobby_id = String::from_utf8(data).unwrap();
                                let mut lobbies = lobbies.lock().await;
                                let lobby = lobbies
                                    .entry(lobby_id.clone())
                                    .and_modify(|v| {
                                        v.add_client(LobbyClient::new(
                                            id,
                                            udp_addr,
                                            receiver.take().unwrap(),
                                        ));
                                    })
                                    .or_insert(Lobby::new_with_client(
                                        lobby_id.clone(),
                                        LobbyClient::new(id, udp_addr, receiver.take().unwrap()),
                                    ));
                                sender.recv_call_ret(&[]).await.unwrap();
                                lobby.notify_lobby_clients().await.unwrap();
                            }
                            RpcCode::Unknown | RpcCode::ReceiveLobbyClients => break,
                        }
                    }
                })
                .detach();
            }
        });
    });
}
