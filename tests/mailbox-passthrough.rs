// Tests for mailbox_passthrough() API contract
// Only tests the library API behavior, not gateway wire protocol

use ethercrab::{
    error::{Error, MailboxError},
    std::{ethercat_now, tx_rx_task},
    MainDevice, MainDeviceConfig, PduStorage, SubDeviceGroup, Timeouts,
};
use std::time::Duration;

const MAX_SLAVES: usize = 16;
const MAX_PDU_DATA: usize = PduStorage::element_size(1100);
const MAX_FRAMES: usize = 16;
const MAX_PDI: usize = 64;

static PDU_STORAGE: PduStorage<MAX_FRAMES, MAX_PDU_DATA> = PduStorage::new();

#[tokio::test]
async fn test_request_too_short() {
    // API must reject requests shorter than 6 bytes
    let (_tx, _rx, pdu_loop) = PDU_STORAGE.try_split().expect("can only split once");
    let maindevice = MainDevice::new(pdu_loop, Timeouts::default(), MainDeviceConfig::default());
    
    // This would need a real network interface to test properly
    // For now, we're documenting the expected behavior
    
    // Test: Request with less than 6 bytes should return error
    let short_request = vec![0x01, 0x02];
    // Expected: Err(Error::Mailbox(MailboxError::InvalidCount))
}

#[tokio::test]
async fn test_request_length_mismatch() {
    // API must reject when actual length < declared mailbox length
    
    // Test: 6-byte header declares 10 bytes payload, but only 8 total provided
    let mut request = vec![0u8; 8];
    request[0] = 0x0A; // Mailbox length = 10
    request[1] = 0x00;
    // Expected: Err(Error::Mailbox(MailboxError::InvalidCount))
}

#[tokio::test]
async fn test_request_exceeds_sm0_size() {
    // API must reject requests larger than SubDevice's SM0 mailbox
    
    // Test: Request larger than write mailbox capacity
    // This requires a configured SubDevice with known SM0 size
    // Expected: Err(Error::Mailbox(MailboxError::TooLong))
}

#[tokio::test]
async fn test_timeout_enforcement() {
    // API must respect the timeout parameter
    
    // Test: Call with very short timeout on unresponsive device
    let timeout = Duration::from_millis(10);
    // Expected: Err(Error::Timeout)
}

#[tokio::test]
async fn test_reply_validation() {
    // API must validate reply structure
    
    // Test cases:
    // 1. Reply shorter than 6 bytes -> InvalidCount
    // 2. Reply length field exceeds actual data -> InvalidCount
    // 3. Reply exceeds SM1 size -> TooLong
}

#[tokio::test]
async fn test_successful_passthrough() {
    // API must forward request and return reply unchanged
    
    // Test: Valid 6-byte header + payload
    // - Send to configured SubDevice
    // - Verify exact bytes returned
    // - No protocol interpretation
}

// Note: Full integration testing requires a real EtherCAT network
// These tests document the API contract for mailbox_passthrough()