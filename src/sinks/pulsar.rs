use crate::{
    buffers::Acker,
    event::{self, Event},
    runtime::TaskExecutor,
    sinks::util::MetadataFuture,
    topology::config::{DataType, SinkConfig, SinkContext, SinkDescription},
};
use futures::{
    future::{self, poll_fn, IntoFuture},
    stream::FuturesUnordered,
    Async, AsyncSink, Future, Poll, Sink, StartSend, Stream,
};
use pulsar::{
    proto::CommandSendReceipt, Consumer, Producer, ProducerOptions, Pulsar, PulsarExecutor,
    SerializeMessage,
};
use serde::{Deserialize, Serialize};
use snafu::{ResultExt, Snafu};
use std::{collections::HashSet, path::PathBuf};

#[derive(Debug, Snafu)]
enum BuildError {
    #[snafu(display("creating pulsar producer failed: {}", source))]
    PulsarSinkFailed { source: pulsar::Error },
    #[snafu(display("invalid path: {:?}", path))]
    InvalidPath { path: PathBuf },
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PulsarSinkConfig {
    address: String,
    topic: String,
    encoding: Encoding,
    batch_size: Option<u32>,
}

#[derive(Deserialize, Serialize, Debug, Eq, PartialEq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum Encoding {
    Text,
    Json,
}

pub struct PulsarSink {
    topic: String,
    encoding: Encoding,
    producer: Producer,
    pulsar: Pulsar,
    in_flight: FuturesUnordered<MetadataFuture<SendFuture, usize>>,
    // ack
    seq_head: usize,
    seq_tail: usize,
    pending_acks: HashSet<usize>,
    acker: Acker,
}

pub type SendFuture =
    Box<dyn Future<Item = CommandSendReceipt, Error = pulsar::Error> + 'static + Send>;

inventory::submit! {
    SinkDescription::new_without_default::<PulsarSinkConfig>("pulsar")
}

#[typetag::serde(name = "pulsar")]
impl SinkConfig for PulsarSinkConfig {
    fn build(&self, cx: SinkContext) -> crate::Result<(super::RouterSink, super::Healthcheck)> {
        Ok((
            Box::new(PulsarSink::new(self.clone(), cx.acker(), cx.exec())?),
            healthcheck(self.clone()),
        ))
    }

    fn input_type(&self) -> DataType {
        DataType::Log
    }

    fn sink_type(&self) -> &'static str {
        "pulsar"
    }
}

impl PulsarSink {
    fn new(config: PulsarSinkConfig, acker: Acker, exec: TaskExecutor) -> crate::Result<Self> {
        let pulsar = Pulsar::new(config.address.parse()?, None, exec).wait()?;
        let producer = pulsar.producer(Some(ProducerOptions {
            batch_size: config.batch_size,
            ..ProducerOptions::default()
        }));

        Ok(Self {
            topic: config.topic,
            encoding: config.encoding,
            pulsar,
            producer,
            in_flight: FuturesUnordered::new(),
            seq_head: 0,
            seq_tail: 0,
            pending_acks: HashSet::new(),
            acker,
        })
    }
}

impl Sink for PulsarSink {
    type SinkItem = Event;
    type SinkError = ();

    fn start_send(&mut self, item: Self::SinkItem) -> StartSend<Self::SinkItem, Self::SinkError> {
        let message = encode_event(item, &self.topic, self.encoding).map_err(|_| ())?;
        let fut = self.producer.send(self.topic.clone(), &PulsarSend(message));

        let seqno = self.seq_head;
        self.seq_head += 1;
        self.in_flight
            .push((Box::new(fut) as SendFuture).join(future::ok(seqno)));
        Ok(AsyncSink::Ready)
    }

    fn poll_complete(&mut self) -> Poll<(), Self::SinkError> {
        loop {
            match self.in_flight.poll() {
                Ok(Async::NotReady) => return Ok(Async::NotReady),
                Ok(Async::Ready(None)) => return Ok(Async::Ready(())),
                Ok(Async::Ready(Some((result, seqno)))) => {
                    trace!(
                        "produced message {:?} from {} at sequence id {}",
                        result.message_id,
                        result.producer_id,
                        result.sequence_id
                    );
                    self.pending_acks.insert(seqno);
                    let mut num_to_ack = 0;
                    while self.pending_acks.remove(&self.seq_tail) {
                        num_to_ack += 1;
                        self.seq_tail += 1;
                    }
                    self.acker.ack(num_to_ack);
                }
                Err(e) => error!("future cancelled: {}", e),
            }
        }
    }
}

// TODO: https://github.com/wyyerd/pulsar-rs/issues/60
struct PulsarSend(Vec<u8>);

impl SerializeMessage for PulsarSend {
    fn serialize_message(input: &Self) -> Result<pulsar::producer::Message, pulsar::Error> {
        Ok(pulsar::producer::Message {
            payload: input.0.clone(),
            ..Default::default()
        })
    }
}

fn encode_event<S: AsRef<str>>(item: Event, topic: S, enc: Encoding) -> crate::Result<Vec<u8>> {
    let log = item.into_log();
    let data = match enc {
        Encoding::Json => serde_json::to_vec(&log.unflatten())?,
        Encoding::Text => log
            .get(&event::MESSAGE)
            .map(|v| v.as_bytes().to_vec())
            .unwrap_or_default(),
    };
    Ok(data)
}

fn healthcheck(config: PulsarSinkConfig) -> super::Healthcheck {
    unimplemented!()
}
