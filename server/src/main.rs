use futures::{channel::mpsc, prelude::*};

use server::{conn, rpc};

async fn async_main() -> anyhow::Result<()> {
    let (mut notify_tx, notify_rx) = mpsc::channel(8);

    let rpc_server = rpc::RpcServer::new(notify_tx.clone());
    let notifier = rpc::Notifier::new(notify_rx);

    let (accept_tx, mut accept_rx) = mpsc::channel(8);
    let tcp_send_receive = conn::TcpSendReceive::new("127.0.0.1", 3000, accept_tx).await?;

    let handler_fut = async {
        loop {
            let (tcp_id, sender, receiver) = match accept_rx.recv().await {
                Ok(v) => v,
                Err(v) => anyhow::bail!(v),
            };
            println!("accepted conn: {:?}", tcp_id);

            notify_tx
                .send(rpc::Notify::NewReceiver(
                    tcp_id,
                    rpc::RpcNotifyClient::new(receiver.into()),
                ))
                .await?;
            let handler = rpc_server.get_handler(tcp_id, sender.into());

            smol::spawn(async move {
                match handler.listen().await {
                    Err(e) => {
                        println!("handler failed: {}", e);
                    }
                    Ok(_) => {}
                }
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
