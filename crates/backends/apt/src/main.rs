use anyhow::Result;
use clap::Parser;
use koca_proto::{
    BackendArgs, BackendSession, Command, ErrorCode, Message, MessageBody, ProtocolError,
    ResultPayload,
};

mod handler;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let args = BackendArgs::parse();
    let mut session = BackendSession::connect(&args.socket).await?;

    // Package names from the most recent `install-plan`, waiting for confirm/abort.
    let mut pending: Option<Vec<String>> = None;

    loop {
        let req = session.recv().await?;
        let id = req.id;

        match req.cmd {
            Command::CheckInstalled { packages } => {
                let body = match handler::check_installed(&packages) {
                    Ok(result) => MessageBody::Result { result },
                    Err(e) => MessageBody::Error { error: e },
                };
                session.send(&Message { id, body }).await?;
            }

            Command::InstallPlan { packages } => match handler::install_plan(&packages) {
                Ok((result, pkg_names)) => {
                    pending = Some(pkg_names);
                    session
                        .send(&Message {
                            id,
                            body: MessageBody::Result { result },
                        })
                        .await?;
                }
                Err(e) => {
                    session
                        .send(&Message {
                            id,
                            body: MessageBody::Error { error: e },
                        })
                        .await?;
                }
            },

            Command::Confirm => {
                if let Some(pkgs) = pending.take() {
                    handler::commit_transaction(id, pkgs, false, &mut session).await;
                } else {
                    session
                        .send(&Message {
                            id,
                            body: MessageBody::Error {
                                error: ProtocolError {
                                    code: ErrorCode::Internal,
                                    message: "no pending transaction to confirm".into(),
                                },
                            },
                        })
                        .await?;
                }
            }

            Command::Abort => {
                pending = None;
                session
                    .send(&Message {
                        id,
                        body: MessageBody::Result {
                            result: ResultPayload::Aborted,
                        },
                    })
                    .await?;
            }

            Command::Remove { packages } => {
                handler::commit_transaction(id, packages, true, &mut session).await;
            }

            Command::Shutdown => break,
        }
    }

    Ok(())
}
