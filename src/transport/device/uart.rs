use super::Shared;
use btleplug::api::{Characteristic, Peripheral, WriteType};
use bytes::Bytes;
use futures_util::stream::Map;
use futures_util::{ready, StreamExt};
use std::io::{Cursor, ErrorKind};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::io::{AsyncBufRead, AsyncRead, AsyncWrite, ReadBuf};
use tokio::select;
use tokio::sync::mpsc::Receiver;
use tokio::sync::mpsc::Sender;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::io::StreamReader;
use tokio_util::sync::ReusableBoxFuture;
use tracing::{debug, trace};

pub struct UartChannel {
    shared: Arc<Shared>,
    mtu: usize,
    tx_characteristic: Characteristic,
    stream_sender: Sender<Sender<Vec<u8>>>,
}

fn recv_map_fn(vec: Vec<u8>) -> std::io::Result<Cursor<Vec<u8>>> {
    Ok(Cursor::new(vec))
}

type RecvMapFnType = fn(Vec<u8>) -> std::io::Result<Cursor<Vec<u8>>>;

impl UartChannel {
    pub(super) fn new(
        shared: Arc<Shared>,
        tx_characteristic: Characteristic,
        _rx_characteristic: Characteristic,
        mut rx_recv: Receiver<Vec<u8>>,
    ) -> Self {
        let (stream_sender, mut stream_reader) = tokio::sync::mpsc::channel::<Sender<Vec<u8>>>(1);

        // spawn a task managing the streams
        tokio::spawn(async move {
            let mut current_stream = None;

            loop {
                select! {
                    new_stream = stream_reader.recv() => {
                        match new_stream {
                            Some(new_stream) => {
                                debug!("A new stream has been opened");
                                current_stream = Some(new_stream);
                            },
                            None => {
                                debug!("The stream_sender has been dropped, stopping the stream manager task");
                                break;
                            }
                        }
                    }
                    data = rx_recv.recv() => {
                        if let Some(stream) = &current_stream {
                            if let Some(data) = data {
                                if stream.send(data).await.is_err() {
                                    debug!("The receiving end of the stream has been dropped, considering it closed");
                                    current_stream = None;
                                }
                            }
                        } else {
                            debug!("Received data but no stream is open, dropping it");
                        }
                    }
                }
            }
        });

        Self {
            shared,
            mtu: 206,
            tx_characteristic,
            stream_sender,
        }
    }

    pub async fn open_stream(&self) -> UartStream {
        let (sender, receiver) = tokio::sync::mpsc::channel::<Vec<u8>>(1);

        self.stream_sender
            .send(sender)
            .await
            .expect("The stream managing task has died?");

        let receiver = ReceiverStream::new(receiver).map(recv_map_fn as RecvMapFnType);
        let reader = StreamReader::new(receiver);

        UartStream {
            shared: self.shared.clone(),
            mtu: self.mtu,
            tx_characteristic: self.tx_characteristic.clone(),
            reader,
            write_finished: true,
            write_box_future: ReusableBoxFuture::new(async move { Ok(()) }),
        }
    }
}

// pin_project! {
pub struct UartStream {
    shared: Arc<Shared>,
    mtu: usize,
    tx_characteristic: Characteristic,
    // #[pin]
    reader: StreamReader<Map<ReceiverStream<Vec<u8>>, RecvMapFnType>, Cursor<Vec<u8>>>,
    write_finished: bool,
    write_box_future: ReusableBoxFuture<'static, btleplug::Result<()>>,
    // #[pin]
    // writer: SinkWriter<
    //     CopyToBytes<
    //         SinkMapErr<
    //             PollSender<
    //                 Bytes
    //             >,
    //             SendMapErrFnType,
    //         >
    //     >
    // >,
}
// }

impl UartStream {
    fn poll_write_ready(&mut self, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        // let mut proj = self.project();

        if !self.write_finished {
            match self.write_box_future.poll(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(e)) => {
                    debug!("Error while writing to the UART: {:?}", e);
                    return Poll::Ready(Err(std::io::Error::new(ErrorKind::BrokenPipe, e)));
                }
                Poll::Ready(Ok(())) => {
                    self.write_finished = true;
                }
            }
        }

        Poll::Ready(Ok(()))
    }
}

impl AsyncRead for UartStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = Pin::into_inner(self);

        ready!(this.poll_write_ready(cx)?);

        Pin::new(&mut this.reader).poll_read(cx, buf)
    }
}

impl AsyncBufRead for UartStream {
    fn poll_fill_buf(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<&[u8]>> {
        let this = Pin::into_inner(self);

        ready!(this.poll_write_ready(cx)?);

        Pin::new(&mut this.reader).poll_fill_buf(cx)
    }

    fn consume(self: Pin<&mut Self>, amt: usize) {
        let this = Pin::into_inner(self);

        Pin::new(&mut this.reader).consume(amt)
    }
}

impl AsyncWrite for UartStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let this = Pin::into_inner(self);

        ready!(this.poll_write_ready(cx)?);

        let buf_len = std::cmp::min(buf.len(), this.mtu);
        let buf = &buf[..buf_len];

        // FIXME: cloning is bad!
        let shared = this.shared.clone();
        let buf = Bytes::copy_from_slice(buf);
        let tx_characteristic = this.tx_characteristic.clone();

        let fut = async move {
            trace!("TX: {}", hex::encode(&buf));
            shared
                .device
                .write(&tx_characteristic, &buf, WriteType::WithoutResponse)
                .await
        };

        this.write_box_future.set(fut);
        this.write_finished = false;

        Poll::Ready(Ok(buf_len))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::into_inner(self).poll_write_ready(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::into_inner(self).poll_write_ready(cx)
    }
}
