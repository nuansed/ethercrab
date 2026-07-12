use crate::{
    error::{Error, MailboxError, TimeoutError},
    register::RegisterAddress,
    subdevice::{SubDevice, SubDeviceRef},
    sync_manager_channel::Status as SmStatus,
};
use core::{ops::Deref, time::Duration};

#[cfg(feature = "std")]
use std::time::Instant;

impl<S> SubDeviceRef<'_, S>
where
    S: Deref<Target = SubDevice>,
{
    /// Raw mailbox passthrough for ETG.8200 gateways.
    ///
    /// Request must start with 6-byte mailbox header.
    /// Returns complete reply (6-byte header + payload).
    /// No protocol interpretation.
    /// Gateway returns no UDP reply on errors.
    pub async fn mailbox_passthrough(
        &self,
        request: &[u8],
        timeout: Duration,
    ) -> Result<Vec<u8>, Error> {
        if request.len() < 6 {
            return Err(Error::Mailbox(MailboxError::InvalidCount));
        }
        let mbox_len = u16::from_le_bytes([request[0], request[1]]) as usize;
        let logical_len = 6 + mbox_len;

        if request.len() < logical_len {
            return Err(Error::Mailbox(MailboxError::InvalidCount));
        }

        let write_mailbox = self
            .config
            .mailbox
            .write
            .ok_or(Error::Mailbox(MailboxError::NoWriteMailbox))?;
        let read_mailbox = self
            .config
            .mailbox
            .read
            .ok_or(Error::Mailbox(MailboxError::NoReadMailbox))?;

        if logical_len > write_mailbox.len as usize {
            return Err(Error::Mailbox(MailboxError::TooLong {
                address: self.configured_address,
                sub_index: 0,
            }));
        }
        #[cfg(feature = "std")]
        let deadline = Instant::now() + timeout;
        #[cfg(not(feature = "std"))]
        let _deadline = ();

        async {
            // Drain stale data from SM1
            let sm1_status_addr = RegisterAddress::sync_manager_status(read_mailbox.sync_manager);
            let mut sm1_status: SmStatus =
                self.read(sm1_status_addr).receive(self.maindevice).await?;
            let mut drain_attempts = 0;
            const MAX_DRAIN_ATTEMPTS: u32 = 10;
            while sm1_status.mailbox_full && drain_attempts < MAX_DRAIN_ATTEMPTS {
                #[cfg(feature = "std")]
                if Instant::now() >= deadline {
                    return Err(Error::Timeout(TimeoutError::MailboxResponse));
                }
                self.read(read_mailbox.address)
                    .ignore_wkc()
                    .receive_slice(self.maindevice, read_mailbox.len)
                    .await?;
                sm1_status = self.read(sm1_status_addr).receive(self.maindevice).await?;
                drain_attempts += 1;
                if sm1_status.mailbox_full {
                    self.maindevice.timeouts.loop_tick().await;
                }
            }

            let sm0_status_addr = RegisterAddress::sync_manager_status(write_mailbox.sync_manager);

            loop {
                #[cfg(feature = "std")]
                if Instant::now() >= deadline {
                    return Err(Error::Timeout(TimeoutError::MailboxResponse));
                }
                let sm0: SmStatus = self.read(sm0_status_addr).receive(self.maindevice).await?;
                if !sm0.mailbox_full {
                    break;
                }
                self.maindevice.timeouts.loop_tick().await;
            }

            // The gateway owns the mailbox counter: external tools restart
            // their own sequence per session, and a counter equal to the
            // slave's last-seen value is treated as a repeat and ignored.
            // Using the shared per-subdevice counter also keeps a single
            // owner alongside EtherCrab's own CoE traffic.
            let mut framed = request[..logical_len].to_vec();
            framed[5] = (framed[5] & 0x0F) | (self.state.mailbox_counter() << 4);

            // The ESC only latches a mailbox write when the last byte of the
            // SM buffer is written, so always write the full mailbox length
            // (mirrors send_coe_service).
            self.write(write_mailbox.address)
                .with_len(write_mailbox.len)
                .send(self.maindevice, framed.as_slice())
                .await?;

            loop {
                #[cfg(feature = "std")]
                if Instant::now() >= deadline {
                    return Err(Error::Timeout(TimeoutError::MailboxResponse));
                }
                let sm1: SmStatus = self.read(sm1_status_addr).receive(self.maindevice).await?;

                if sm1.mailbox_full {
                    let response = self
                        .read(read_mailbox.address)
                        .receive_slice(self.maindevice, read_mailbox.len)
                        .await?;

                    let reply = response.as_ref();

                    if reply.len() < 6 {
                        return Err(Error::Mailbox(MailboxError::InvalidCount));
                    }

                    let reply_len = u16::from_le_bytes([reply[0], reply[1]]) as usize;
                    let total_len = 6 + reply_len;

                    if total_len > reply.len() {
                        return Err(Error::Mailbox(MailboxError::InvalidCount));
                    }

                    if total_len > read_mailbox.len as usize {
                        return Err(Error::Mailbox(MailboxError::TooLong {
                            address: self.configured_address,
                            sub_index: 0,
                        }));
                    }

                    let mut result = vec![0u8; total_len];
                    result.copy_from_slice(&reply[..total_len]);
                    return Ok(result);
                }

                self.maindevice.timeouts.loop_tick().await;
            }
        }
        .await
    }
}
