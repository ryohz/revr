use std::{
    io::{BufReader, Read, Write},
    net::TcpStream,
};

use anyhow::{anyhow, bail, Result};
use clap::Parser;
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    terminal::{disable_raw_mode, enable_raw_mode},
};
use tokio::{select, sync::watch, task::JoinHandle};

use crate::{
    command::ArgsParser,
    revshell::session::{Session, SESSIONS},
};

#[derive(Parser)]
struct Args {
    id: Option<usize>,
}

pub async fn sessions(args: &[&str]) -> Result<()> {
    let args = Args::parse_args("sessions", args)?;

    if args.id.is_none() {
        use prettytable::{row, Table};

        let sessions = super::revshell::session::get_sessions();
        let mut table = Table::new();

        table.add_row(row!["id", "address"]);
        for session in sessions {
            table.add_row(row![
                session.id.to_string(),
                session.remote_addr.to_string()
            ]);
        }

        println!("{}", table);
        return Ok(());
    }

    let id = args.id.unwrap();
    start(id).await.unwrap();

    Ok(())
}


pub async fn start(id: usize) -> Result<()> {
    let session = {
        let mut sessions = SESSIONS.lock().unwrap();

        let mut index = None;
        for (i, s) in sessions.iter().enumerate() {
            if s.metadata.id == id {
                index = Some(i);
            }
        }
        if index.is_none() {
            bail!(anyhow!("session with id {} was not found", id));
        }

        sessions.remove(index.unwrap())
    };

    let saved_tcp_stream = session.tcp_stream.try_clone().unwrap();
    // let mut writer = session.tcp_stream.try_clone().unwrap();
    let (sender, recver) = watch::channel(());

    let t1 = stdout_stream_pipe(session.tcp_stream.try_clone().unwrap(), recver).await;
    let t2 = stdin_stream_pipe(session.tcp_stream.try_clone().unwrap(), sender).await;
    t1.await?;
    t2.await?;
    // println!("hello");

    let session = Session {
        tcp_stream: saved_tcp_stream,
        metadata: session.metadata,
    };

    let mut sessions = SESSIONS.lock().unwrap();
    sessions.push(session);

    Ok(())
}

pub async fn stdout_stream_pipe(stream: TcpStream, recver: watch::Receiver<()>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut buffer = [0; 1024];
        let mut recver = recver;
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(1)))
            .unwrap();
        let mut reader = BufReader::new(stream);

        async fn read(reader: &mut BufReader<TcpStream>, buffer: &mut [u8]) -> Result<usize> {
            let len = reader.read(buffer)?;
            Ok(len)
        }

        loop {
            select! {
                _ = recver.changed() => {
                    break
                }

                result = read(&mut reader, &mut buffer) => {
                match result
                 {
                    Ok(0) => {
                        break;
                    }
                    Ok(n) => {
                        std::io::stdout().write_all(&buffer[..n]).unwrap();
                        std::io::stdout().flush().unwrap();
                    }
                    Err(e) => {
                        println!("{}", e);
                        continue;
                    }
                }}
            }
        }
    })
}

pub async fn stdin_stream_pipe(
    stream: TcpStream,
    sender: watch::Sender<()>,
) -> JoinHandle<Result<&'static str>> {
    tokio::spawn(async move {
        let mut writer = stream;
        enable_raw_mode()?;

        'pipe_loop: loop {
            if event::poll(std::time::Duration::from_millis(500))? {
                if let Event::Key(KeyEvent {
                    code,
                    modifiers,
                    kind,
                    state,
                }) = event::read()?
                {
                    match code {
                        KeyCode::Char(c) => {
                            let mut key_sequence = vec![c as u8];
                            if modifiers.contains(KeyModifiers::CONTROL) {
                                key_sequence.insert(0, 0x1b);
                            }
                            if key_sequence == vec![0x1b, 100] {
                                sender.send(())?;
                                break 'pipe_loop;
                            }
                            writer.write_all(&key_sequence)?;
                        }
                        KeyCode::Enter => writer.write_all(b"\n")?,
                        KeyCode::Backspace => writer.write_all(b"\x08")?,
                        KeyCode::Esc => writer.write_all(b"\x1b")?,
                        _ => {}
                    }
                }
            }
        }
        disable_raw_mode()?;
        Ok("")
    })
}
