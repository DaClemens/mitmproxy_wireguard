use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::Result;
use pyo3::{prelude::*, types::PyBytes};
use tokio::sync::{broadcast::Receiver as BroadcastReceiver, mpsc, Mutex};
use tokio::task::JoinHandle;

use super::{socketaddr_to_py, TcpStream};
use crate::messages::{TransportCommand, TransportEvent};

pub struct PyInteropTask {
    local_addr: SocketAddr,
    py_loop: PyObject,
    py_to_smol_tx: mpsc::UnboundedSender<TransportCommand>,
    smol_to_py_rx: mpsc::Receiver<TransportEvent>,
    py_tcp_handler: PyObject,
    py_udp_handler: PyObject,
    py_other_packet_handler: PyObject,
    sd_watcher: BroadcastReceiver<()>,
}

impl PyInteropTask {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        local_addr: SocketAddr,
        py_loop: PyObject,
        py_to_smol_tx: mpsc::UnboundedSender<TransportCommand>,
        smol_to_py_rx: mpsc::Receiver<TransportEvent>,
        py_tcp_handler: PyObject,
        py_udp_handler: PyObject,
        py_other_packet_handler: PyObject,
        sd_watcher: BroadcastReceiver<()>,
    ) -> Self {
        PyInteropTask {
            local_addr,
            py_loop,
            py_to_smol_tx,
            smol_to_py_rx,
            py_tcp_handler,
            py_udp_handler,
            py_other_packet_handler,
            sd_watcher,
        }
    }

    pub async fn run(mut self) -> Result<()> {
        let task_counter = Arc::new(AtomicUsize::new(0));
        let handler_tasks = Arc::new(Mutex::new(HashMap::new()));

        loop {
            tokio::select!(
                // wait for graceful shutdown
                _ = self.sd_watcher.recv() => break,
                // wait for network events
                event = self.smol_to_py_rx.recv() => {
                    if let Some(event) = event {
                        match event {
                            TransportEvent::ConnectionEstablished {
                                connection_id,
                                src_addr,
                                dst_addr,
                                src_orig,
                            } => {
                                // initialize new TCP stream
                                let stream = TcpStream {
                                    connection_id,
                                    event_tx: self.py_to_smol_tx.clone(),
                                    peername: src_addr,
                                    sockname: self.local_addr,
                                    original_dst: dst_addr,
                                    original_src: src_orig,
                                    is_closing: false,
                                };

                                // spawn TCP connection handler coroutine
                                match Python::with_gil(|py| -> Result<(usize, JoinHandle<()>), PyErr> {
                                    let stream = stream.into_py(py);

                                    // calling Python coroutine object yields an awaitable object
                                    let coro = self.py_tcp_handler.call1(py, (stream, ))?;

                                    // convert Python awaitable into Rust Future
                                    let locals = pyo3_asyncio::TaskLocals::new(self.py_loop.as_ref(py))
                                        .copy_context(self.py_loop.as_ref(py).py())?;
                                    let future = pyo3_asyncio::into_future_with_locals(&locals, coro.as_ref(py))?;

                                    let tasklist = Arc::clone(&handler_tasks);
                                    let task_id = task_counter.fetch_add(1, Ordering::Relaxed);

                                    // run Future on a new Tokio task
                                    let handle = tokio::spawn(async move {
                                        if let Err(err) = future.await {
                                            log::error!("TCP connection handler coroutine raised an exception:\n{}", err)
                                        }

                                        let mut tasks = tasklist.lock().await;
                                        tasks.remove(&task_id);
                                    });

                                    Ok((task_id, handle))
                                }) {
                                    Err(err) => log::error!("Failed to spawn TCP connection handler coroutine:\n{}", err),
                                    Ok((task_id, handle)) => {
                                        let mut tasks = handler_tasks.lock().await;
                                        tasks.insert(task_id, handle);
                                    }
                                }
                            },
                            TransportEvent::DatagramReceived {
                                data,
                                src_addr,
                                dst_addr,
                                ..
                            } => {
                                match Python::with_gil(|py| -> Result<(usize, JoinHandle<()>), PyErr> {
                                    let bytes: Py<PyBytes> = PyBytes::new(py, &data).into_py(py);

                                    let coro = self.py_udp_handler.call1(py, (bytes, socketaddr_to_py(py, src_addr), socketaddr_to_py(py, dst_addr)))?;

                                    // convert Python awaitable into Rust Future
                                    let locals = pyo3_asyncio::TaskLocals::new(self.py_loop.as_ref(py))
                                        .copy_context(self.py_loop.as_ref(py).py())?;
                                    let future = pyo3_asyncio::into_future_with_locals(&locals, coro.as_ref(py))?;

                                    let tasklist = Arc::clone(&handler_tasks);
                                    let task_id = task_counter.fetch_add(1, Ordering::Relaxed);

                                    // run Future on a new Tokio task
                                    let handle = tokio::spawn(async move {
                                        if let Err(err) = future.await {
                                            log::error!("UDP handler coroutine raised an exception:\n{}", err)
                                        }

                                        let mut tasks = tasklist.lock().await;
                                        tasks.remove(&task_id);
                                    });

                                    Ok((task_id, handle))
                                }) {
                                    Err(err) => log::error!("Failed to spawn UDP handler coroutine:\n{}", err),
                                    Ok((task_id, handle)) => {
                                        let mut tasks = handler_tasks.lock().await;
                                        tasks.insert(task_id, handle);
                                    }
                                }
                            },
                            TransportEvent::OtherPacketReceived {
                                data,
                                ..
                            } => {
                                match Python::with_gil(|py| -> Result<(usize, JoinHandle<()>), PyErr> {
                                    let bytes: Py<PyBytes> = PyBytes::new(py, &data).into_py(py);

                                    let coro = self.py_other_packet_handler.call1(py, (bytes, ))?;

                                    // convert Python awaitable into Rust Future
                                    let locals = pyo3_asyncio::TaskLocals::new(self.py_loop.as_ref(py))
                                        .copy_context(self.py_loop.as_ref(py).py())?;
                                    let future = pyo3_asyncio::into_future_with_locals(&locals, coro.as_ref(py))?;

                                    let tasklist = Arc::clone(&handler_tasks);
                                    let task_id = task_counter.fetch_add(1, Ordering::Relaxed);

                                    // run Future on a new Tokio task
                                    let handle = tokio::spawn(async move {
                                        if let Err(err) = future.await {
                                            log::error!("Other packet handler coroutine raised an exception:\n{}", err)
                                        }

                                        let mut tasks = tasklist.lock().await;
                                        tasks.remove(&task_id);
                                    });

                                    Ok((task_id, handle))
                                }) {
                                    Err(err) => log::error!("Failed to spawn other packet handler coroutine:\n{}", err),
                                    Ok((task_id, handle)) => {
                                        let mut tasks = handler_tasks.lock().await;
                                        tasks.insert(task_id, handle);
                                    }
                                }
                            },
                        }
                    } else {
                        // channel was closed
                        break;
                    }
                },
            );
        }

        log::debug!("Python interoperability task shutting down.");

        let mut tasks = handler_tasks.lock().await;
        // clean up TCP connection handler coroutines
        for (_, handle) in &mut *tasks {
            if handle.is_finished() {
                // Future is already finished: just await;
                // Python exceptions are already logged by the wrapper coroutine
                if let Err(err) = handle.await {
                    log::warn!("Python handler coroutine could not be joined: {}", err);
                }
            } else {
                // Future is not finished: abort tokio task
                handle.abort();

                if let Err(err) = handle.await {
                    if !err.is_cancelled() {
                        // JoinError was not caused by cancellation: coroutine panicked, log error
                        log::error!("Python handler coroutine panicked: {}", err);
                    }
                }
            }
        }

        Ok(())
    }
}
