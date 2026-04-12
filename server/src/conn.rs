use std::{
    collections::HashMap,
    fs,
    io::{self, prelude::*},
};

use futures::{channel::mpsc, prelude::*};
use smol::net::{TcpListener, TcpStream};

fn rand_bytes(buf: &mut [u8]) -> io::Result<()> {
    let mut dev_random = fs::File::open("/dev/urandom")?;
    dev_random.read_exact(buf)
}

pub type TcpId = [u8; 32];

pub type TcpSenderReceiver = (TcpId, TcpStream, TcpStream);

pub struct TcpSendReceive {
    sub_listener: TcpListener,
    rpc_listener: TcpListener,
    id_map: crate::ArcMu<HashMap<TcpId, TcpStream>>,
    accept_tx: mpsc::Sender<TcpSenderReceiver>,
}

impl TcpSendReceive {
    pub async fn new(
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
            id_map: crate::arcmu(HashMap::new()),
        })
    }

    pub async fn listen(self) -> anyhow::Result<()> {
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
