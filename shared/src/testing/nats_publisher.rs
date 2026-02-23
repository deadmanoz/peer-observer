pub struct NatsPublisherForTesting {
    client: async_nats::Client,
}

impl NatsPublisherForTesting {
    pub async fn new(port: u16) -> Self {
        let addr = format!("127.0.0.1:{}", port);
        log::debug!("The testing NATS publisher is connecting to {}..", addr);
        Self {
            client: async_nats::connect(addr)
                .await
                .expect("should be able to connect to NATS server"),
        }
    }

    pub async fn publish(&self, subject: String, payload: Vec<u8>) {
        self.client
            .publish(subject, payload.into())
            .await
            .expect("should be able to publish");
    }

    /// Flush the NATS client connection, ensuring all published messages have
    /// been received by the server (PING/PONG round-trip).
    pub async fn flush(&self) {
        self.client
            .flush()
            .await
            .expect("should be able to flush NATS client");
    }
}
