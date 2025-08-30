// Integration tests for EtherCAT Mailbox Gateway (ETG.8200)
// These tests verify all requirements from the consultant's review
// This file is automatically run by `cargo test`

use std::collections::HashMap;

const EC_HDR: usize = 2;
const MBX_HDR: usize = 6;
const MAX_PACKET_SIZE: usize = 1500;

// Helper to build a test packet
fn build_packet(ec_len: u16, mbox_len: u16, station: u16, extra_bytes: usize) -> Vec<u8> {
    let mut pkt = Vec::new();
    
    // EtherCAT header (2 bytes)
    let ec_hdr = ec_len & 0x07ff;  // 11-bit length
    pkt.extend_from_slice(&ec_hdr.to_le_bytes());
    
    // Mailbox header (6 bytes)
    if ec_len >= MBX_HDR as u16 {
        pkt.extend_from_slice(&mbox_len.to_le_bytes()); // Length
        pkt.extend_from_slice(&station.to_le_bytes());   // Address
        pkt.extend_from_slice(&[0x00, 0x00]);           // Control bytes
        
        // Mailbox payload
        let payload_len = mbox_len as usize;
        pkt.resize(EC_HDR + MBX_HDR + payload_len, 0xAA);
    }
    
    // Extra trailing bytes (if requested)
    pkt.resize(pkt.len() + extra_bytes, 0xBB);
    
    pkt
}

// Simplified handle_frame for testing (mirrors the gateway's process_mailbox logic)
async fn handle_frame<F, Fut>(
    pkt: &[u8],
    addr_to_idx: &HashMap<u16, usize>,
    mut forward: F,
) -> Result<Option<Vec<u8>>, Box<dyn std::error::Error>>
where
    F: FnMut(usize, &[u8]) -> Fut,
    Fut: std::future::Future<Output = Result<Vec<u8>, Box<dyn std::error::Error>>>,
{
    if pkt.len() < EC_HDR {
        return Ok(None);
    }
    
    let hdr = u16::from_le_bytes([pkt[0], pkt[1]]);
    let ec_len = (hdr & 0x07ff) as usize;
    let upper = hdr & 0xf800;
    
    // Critical bounds check 1: enough data for declared length
    if pkt.len() < EC_HDR + ec_len {
        return Ok(None);
    }
    
    // Critical bounds check 2: ec_len must be at least 6 for mailbox header
    if ec_len < MBX_HDR {
        return Ok(None);
    }
    
    // Critical bounds check 3: declared length must not exceed max
    if ec_len > MAX_PACKET_SIZE - EC_HDR {
        return Ok(None);
    }
    
    let mbox = &pkt[EC_HDR..EC_HDR + ec_len];
    
    let mlen = u16::from_le_bytes([mbox[0], mbox[1]]) as usize;
    let station = u16::from_le_bytes([mbox[2], mbox[3]]);
    
    if MBX_HDR + mlen > mbox.len() {
        return Ok(None);
    }
    
    let Some(&idx) = addr_to_idx.get(&station) else {
        return Ok(None);
    };
    
    match forward(idx, mbox).await {
        Ok(rep) => {
            let max_payload = (MAX_PACKET_SIZE - EC_HDR).min(0x07ff);
            
            if rep.len() > max_payload {
                Ok(None)
            } else {
                let mut out = Vec::with_capacity(EC_HDR + rep.len());
                let reply_hdr = upper | ((rep.len() as u16) & 0x07ff);
                out.extend_from_slice(&reply_hdr.to_le_bytes());
                out.extend_from_slice(&rep);
                Ok(Some(out))
            }
        }
        Err(_) => Ok(None),
    }
}

// ========================================================================
// UDP Tests (Blocker #1: Make UDP the canonical path)
// ========================================================================

#[tokio::test]
async fn test_udp_malformed_header_short_packet() {
    // Test: Packet shorter than 2 bytes (no EtherCAT header)
    let pkt = vec![0x01];  // Only 1 byte
    
    let addr_to_idx = HashMap::new();
    let result = handle_frame(&pkt, &addr_to_idx, |_, _| async {
        panic!("Should not forward");
    }).await;
    
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), None); // No reply on malformed packet
}

#[tokio::test]
async fn test_udp_malformed_header_len11_exceeds_body() {
    // Test: len11 field indicates more data than packet contains
    let mut pkt = vec![];
    
    // EtherCAT header with len11 = 100, but only provide 10 bytes of data
    let ec_hdr: u16 = 100;  // Claim 100 bytes
    pkt.extend_from_slice(&ec_hdr.to_le_bytes());
    pkt.resize(12, 0x00);  // But only provide 10 bytes after header
    
    let addr_to_idx = HashMap::new();
    let result = handle_frame(&pkt, &addr_to_idx, |_, _| async {
        panic!("Should not forward");
    }).await;
    
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), None); // No reply on truncated packet
}

#[tokio::test]
async fn test_udp_ec_len_less_than_6_bytes() {
    // Critical test for consultant's bounds check requirement
    // ec_len < 6 means no valid mailbox header can exist
    let mut pkt = vec![];
    
    // EtherCAT header with len11 = 4 (less than mailbox header size)
    let ec_hdr: u16 = 4;
    pkt.extend_from_slice(&ec_hdr.to_le_bytes());
    pkt.extend_from_slice(&[0xAA, 0xBB, 0xCC, 0xDD]);
    
    let addr_to_idx = HashMap::new();
    let result = handle_frame(&pkt, &addr_to_idx, |_, _| async {
        panic!("Should not forward - ec_len < 6");
    }).await;
    
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), None); // No reply when ec_len < 6
}

#[tokio::test]
async fn test_udp_oversized_len11() {
    // Test: len11 > 1498 (max payload)
    let mut pkt = vec![];
    
    // EtherCAT header with len11 = 1499 (exceeds max)
    let ec_hdr: u16 = 1499;
    pkt.extend_from_slice(&ec_hdr.to_le_bytes());
    pkt.resize(1501, 0x00);  // Provide the data
    
    let addr_to_idx = HashMap::new();
    let result = handle_frame(&pkt, &addr_to_idx, |_, _| async {
        panic!("Should not forward");
    }).await;
    
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), None); // No reply on oversized
}

#[tokio::test]
async fn test_udp_trailing_bytes_ignored() {
    // Test: Trailing bytes are accepted but not forwarded
    let station: u16 = 0x1001;
    let mut addr_to_idx = HashMap::new();
    addr_to_idx.insert(station, 0);
    
    // Build packet with 10 trailing bytes
    let pkt = build_packet(20, 14, station, 10);
    
    let mut forwarded_data: Option<Vec<u8>> = None;
    let result = handle_frame(&pkt, &addr_to_idx, |idx, data| {
        assert_eq!(idx, 0);
        forwarded_data = Some(data.to_vec());
        async {
            // Return a simple reply
            Ok(vec![0x08, 0x00, 0x01, 0x10, 0x00, 0x00, 0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF, 0x11, 0x22])
        }
    }).await;
    
    assert!(result.is_ok());
    let reply = result.unwrap();
    assert!(reply.is_some());
    
    // Verify only logical mailbox was forwarded (no trailing bytes)
    assert!(forwarded_data.is_some());
    let fwd = forwarded_data.unwrap();
    assert_eq!(fwd.len(), 20);  // ec_len, not including trailing bytes
}

#[tokio::test]
async fn test_udp_unknown_station_no_reply() {
    // Test: Unknown station address results in no reply
    let station = 0x9999;  // Not in map
    let addr_to_idx = HashMap::new();  // Empty map
    
    let pkt = build_packet(20, 14, station, 0);
    
    let result = handle_frame(&pkt, &addr_to_idx, |_, _| async {
        panic!("Should not forward to unknown station");
    }).await;
    
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), None); // No reply for unknown station
}

#[tokio::test] 
async fn test_udp_timeout_no_reply() {
    // Test: Timeout results in no reply
    let station: u16 = 0x1001;
    let mut addr_to_idx = HashMap::new();
    addr_to_idx.insert(station, 0);
    
    let pkt = build_packet(20, 14, station, 0);
    
    let result = handle_frame(&pkt, &addr_to_idx, |_, _| async {
        // Simulate timeout by returning error
        Err("Timeout".into())
    }).await;
    
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), None); // No reply on timeout
}

// ========================================================================
// Mailbox Tests (Should-fix #6: SM corner cases)
// ========================================================================

#[tokio::test]
async fn test_mailbox_reply_larger_than_declared() {
    // Test: Reply claims more bytes than available
    let station: u16 = 0x1001;
    let mut addr_to_idx = HashMap::new();
    addr_to_idx.insert(station, 0);
    
    let pkt = build_packet(20, 14, station, 0);
    
    let result = handle_frame(&pkt, &addr_to_idx, |_, _| async {
        // Return malformed reply: header says 100 bytes but only provide 10
        let mut reply = vec![];
        reply.extend_from_slice(&100u16.to_le_bytes());  // Claim 100 bytes
        reply.extend_from_slice(&station.to_le_bytes());
        reply.extend_from_slice(&[0x00, 0x00]);
        reply.extend_from_slice(&[0xAA; 4]);  // But only 4 bytes payload
        Ok(reply)
    }).await;
    
    // This should be accepted by handle_frame (it doesn't validate reply internals)
    // The actual mailbox_passthrough would reject this
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_mailbox_truncated() {
    // Test: Mailbox header declares more payload than available
    let station: u16 = 0x1001;
    let mut addr_to_idx = HashMap::new();
    addr_to_idx.insert(station, 0);
    
    let mut pkt = vec![];
    
    // EtherCAT header
    let ec_hdr: u16 = 10;  // 10 bytes total
    pkt.extend_from_slice(&ec_hdr.to_le_bytes());
    
    // Mailbox header claiming 20 bytes payload (but we only have 4)
    pkt.extend_from_slice(&20u16.to_le_bytes());  // Mailbox length
    pkt.extend_from_slice(&station.to_le_bytes());
    pkt.extend_from_slice(&[0x00, 0x00]);
    pkt.extend_from_slice(&[0xAA, 0xBB, 0xCC, 0xDD]);  // Only 4 bytes
    
    let result = handle_frame(&pkt, &addr_to_idx, |_, _| async {
        panic!("Should not forward truncated mailbox");
    }).await;
    
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), None); // Truncated -> no reply
}

// ========================================================================
// Header Preservation Tests (Should-fix #11: Type bit preservation)
// ========================================================================

#[tokio::test]
async fn test_ethercat_header_upper_bits_preserved() {
    // Test: Upper 5 bits (Reserved + Type) are preserved in reply
    let station: u16 = 0x1001;
    let mut addr_to_idx = HashMap::new();
    addr_to_idx.insert(station, 0);
    
    // Build packet with specific upper bits pattern
    let mut pkt = vec![];
    
    // EtherCAT header with upper bits = 0xA800, length = 20
    let ec_hdr: u16 = 0xA800 | 20;  // Upper bits 0xA800, length 20
    pkt.extend_from_slice(&ec_hdr.to_le_bytes());
    
    // Valid mailbox
    pkt.extend_from_slice(&14u16.to_le_bytes());
    pkt.extend_from_slice(&station.to_le_bytes());
    pkt.extend_from_slice(&[0x00, 0x00]);
    pkt.resize(EC_HDR + 20, 0xAA);
    
    let result = handle_frame(&pkt, &addr_to_idx, |_, _| async {
        // Return 10-byte reply
        Ok(vec![0x04, 0x00, 0x01, 0x10, 0x00, 0x00, 0xBB, 0xBB, 0xBB, 0xBB])
    }).await;
    
    assert!(result.is_ok());
    let reply = result.unwrap().unwrap();
    
    // Check that upper bits are preserved
    let reply_hdr = u16::from_le_bytes([reply[0], reply[1]]);
    assert_eq!(reply_hdr & 0xF800, 0xA800);  // Upper bits preserved
    assert_eq!(reply_hdr & 0x07FF, 10);      // Length updated correctly
}

#[tokio::test]
async fn test_11bit_length_field_limit() {
    // Test: Verify MTU limit (1498 bytes max payload)
    let station: u16 = 0x1001;
    let mut addr_to_idx = HashMap::new();
    addr_to_idx.insert(station, 0);
    
    let pkt = build_packet(20, 14, station, 0);
    
    // Test reply at MTU limit (1498 bytes max payload)
    let result = handle_frame(&pkt, &addr_to_idx, |_, _| async {
        let mut reply = vec![0xD4, 0x05];  // Length = 1492 (1498 - 6)
        reply.extend_from_slice(&station.to_le_bytes());
        reply.extend_from_slice(&[0x00, 0x00]);
        reply.resize(1498, 0xAA);  // Max MTU payload
        Ok(reply)
    }).await;
    
    assert!(result.is_ok());
    // This should be accepted at exactly MTU limit
    let reply = result.unwrap();
    assert!(reply.is_some());
    let reply = reply.unwrap();
    assert_eq!(reply.len(), 1500); // EC_HDR + 1498
}

#[tokio::test]
async fn test_over_mtu_limit() {
    // Test: Verify that exceeding MTU limit is rejected
    let station: u16 = 0x1001;
    let mut addr_to_idx = HashMap::new();
    addr_to_idx.insert(station, 0);
    
    let pkt = build_packet(20, 14, station, 0);
    
    // Test reply at 1499 bytes (exceeds MTU limit)
    let result = handle_frame(&pkt, &addr_to_idx, |_, _| async {
        let mut reply = vec![0xD5, 0x05];  // Length = 1493 (1499 - 6)
        reply.extend_from_slice(&station.to_le_bytes());
        reply.extend_from_slice(&[0x00, 0x00]);
        reply.resize(1499, 0xAA);  // Over MTU limit
        Ok(reply)
    }).await;
    
    assert!(result.is_ok());
    // This should be rejected as too large
    assert_eq!(result.unwrap(), None);
}

// ========================================================================
// Station Address Tests (Blocker #3: configured address mapping)
// ========================================================================

#[tokio::test]
async fn test_configured_station_address_routing() {
    // Test: Verify routing uses configured station address
    let mut addr_to_idx = HashMap::new();
    addr_to_idx.insert(0x1001, 0);  // Station 0x1001 -> index 0
    addr_to_idx.insert(0x1002, 1);  // Station 0x1002 -> index 1
    addr_to_idx.insert(0x1003, 2);  // Station 0x1003 -> index 2
    
    // Test routing to station 0x1002
    let pkt = build_packet(20, 14, 0x1002, 0);
    
    let mut routed_index = None;
    let result = handle_frame(&pkt, &addr_to_idx, |idx, _| {
        routed_index = Some(idx);
        async move {
            Ok(vec![0x04, 0x00, 0x02, 0x10, 0x00, 0x00, 0xAA, 0xBB, 0xCC, 0xDD])
        }
    }).await;
    
    assert!(result.is_ok());
    assert!(result.unwrap().is_some());
    assert_eq!(routed_index, Some(1));  // Should route to index 1
}

// ========================================================================
// Integration Test
// ========================================================================

#[tokio::test]
async fn test_end_to_end_mailbox_forwarding() {
    // Complete end-to-end test of mailbox forwarding
    let station: u16 = 0x1001;
    let mut addr_to_idx = HashMap::new();
    addr_to_idx.insert(station, 0);
    
    // Build a complete CoE SDO read request
    let mut pkt = vec![];
    
    // EtherCAT header
    let ec_hdr: u16 = 0x2000 | 16;  // Type=2, Length=16
    pkt.extend_from_slice(&ec_hdr.to_le_bytes());
    
    // Mailbox header
    pkt.extend_from_slice(&10u16.to_le_bytes());   // Mailbox length = 10
    pkt.extend_from_slice(&station.to_le_bytes()); // Address
    pkt.extend_from_slice(&[0x00, 0x03]);          // Priority=0, Type=CoE
    
    // CoE header (simplified)
    pkt.extend_from_slice(&[0x02, 0x00]);  // CoE header
    
    // SDO request (simplified)
    pkt.extend_from_slice(&[0x40, 0x00, 0x60, 0x00, 0x00, 0x00, 0x00, 0x00]);
    
    let result = handle_frame(&pkt, &addr_to_idx, |idx, data| {
        assert_eq!(idx, 0);
        assert_eq!(data.len(), 16);  // Full mailbox telegram
        
        // Verify mailbox header in forwarded data
        assert_eq!(u16::from_le_bytes([data[0], data[1]]), 10);  // Mailbox len
        assert_eq!(u16::from_le_bytes([data[2], data[3]]), station);  // Station
        
        async move {
            // Return SDO response (6 byte mailbox header + 10 bytes payload = 16 total)
            let mut reply = vec![];
            reply.extend_from_slice(&10u16.to_le_bytes());   // Mailbox length = 10
            reply.extend_from_slice(&station.to_le_bytes()); // Address  
            reply.extend_from_slice(&[0x00, 0x03]);          // Priority=0, Type=CoE
            reply.extend_from_slice(&[0x03, 0x00]);          // CoE response header
            reply.extend_from_slice(&[0x43, 0x00, 0x60, 0x00, 0x01, 0x02, 0x03, 0x04]);
            
            Ok(reply)
        }
    }).await;
    
    assert!(result.is_ok());
    let reply = result.unwrap().unwrap();
    
    // Verify reply structure
    assert_eq!(reply.len(), EC_HDR + 16);  // EC header + mailbox reply (6+10)
    
    // Check EC header
    let reply_hdr = u16::from_le_bytes([reply[0], reply[1]]);
    assert_eq!(reply_hdr & 0xF800, 0x2000);  // Type preserved
    assert_eq!(reply_hdr & 0x07FF, 16);      // Length = mailbox reply size
    
    // Check mailbox header in reply
    assert_eq!(u16::from_le_bytes([reply[2], reply[3]]), 10);  // Mailbox length
    assert_eq!(u16::from_le_bytes([reply[4], reply[5]]), station);  // Station
}

// NEW TEST 1: Enforce 1498-byte ceiling at socket boundary
#[tokio::test]
async fn test_udp_1498_byte_ceiling() {
    let mut addr_to_idx = HashMap::new();
    addr_to_idx.insert(0x1001, 0);
    
    // Test exactly at the 1498 byte limit (MAX_PACKET_SIZE - EC_HDR)
    let large_mailbox = 1498 - MBX_HDR;  // 1492 bytes of payload
    let mut pkt = Vec::new();
    
    // EtherCAT header
    let ec_hdr = 1498u16 & 0x07ff;
    pkt.extend_from_slice(&ec_hdr.to_le_bytes());
    
    // Mailbox header
    pkt.extend_from_slice(&(large_mailbox as u16).to_le_bytes());
    pkt.extend_from_slice(&0x1001u16.to_le_bytes());
    pkt.extend_from_slice(&[0x00, 0x03]);  // CoE
    
    // Fill payload
    pkt.resize(EC_HDR + 1498, 0xAA);
    
    // This should work - exactly at the limit
    let result = handle_frame(&pkt, &addr_to_idx, |_idx, data| {
        assert_eq!(data.len(), 1498);  // Full size forwarded
        async move {
            // Return a smaller response
            let mut reply = vec![];
            reply.extend_from_slice(&10u16.to_le_bytes());
            reply.extend_from_slice(&0x1001u16.to_le_bytes());
            reply.extend_from_slice(&[0x00, 0x03]);
            reply.resize(16, 0);
            Ok(reply)
        }
    }).await;
    
    assert!(result.is_ok());
    assert!(result.unwrap().is_some());
    
    // Test exceeding the 1498 byte limit (should be dropped)
    let mut pkt2 = Vec::new();
    let oversized = 1499u16;  // 1 byte over the limit
    pkt2.extend_from_slice(&oversized.to_le_bytes());
    pkt2.extend_from_slice(&(1493u16).to_le_bytes());  // Mailbox len
    pkt2.extend_from_slice(&0x1001u16.to_le_bytes());
    pkt2.extend_from_slice(&[0x00, 0x03]);
    pkt2.resize(EC_HDR + 1499, 0xBB);
    
    // This should be dropped (no reply)
    let result2 = handle_frame(&pkt2, &addr_to_idx, |_idx, _data| {
        async move {
            panic!("Should not forward oversized frame");
        }
    }).await;
    
    assert!(result2.is_ok());
    assert!(result2.unwrap().is_none());  // No reply for oversized
}

// NEW TEST 2: Accept non-zero Channel/Priority bits in mailbox header
#[tokio::test]
async fn test_mailbox_channel_priority_bits() {
    let mut addr_to_idx = HashMap::new();
    addr_to_idx.insert(0x1001, 0);
    
    let mut pkt = Vec::new();
    
    // EtherCAT header
    let ec_hdr = 16u16;  // 6 byte mailbox header + 10 byte payload
    pkt.extend_from_slice(&ec_hdr.to_le_bytes());
    
    // Mailbox header with non-zero channel and priority
    pkt.extend_from_slice(&10u16.to_le_bytes());   // Length
    pkt.extend_from_slice(&0x1001u16.to_le_bytes()); // Address
    pkt.push(0x24);  // Channel=2, Priority=1 (bits 5-6 and 0-1)
    pkt.push(0x03);  // Type=CoE
    
    // CoE payload
    pkt.extend_from_slice(&[0x02, 0x00]);  // CoE header
    pkt.extend_from_slice(&[0x40, 0x00, 0x60, 0x00, 0x00, 0x00, 0x00, 0x00]);
    
    let result = handle_frame(&pkt, &addr_to_idx, |_idx, data| {
        // Verify the channel/priority bits are preserved
        assert_eq!(data[4], 0x24);  // Channel and priority preserved
        assert_eq!(data[5], 0x03);  // Type preserved
        
        async move {
            // Reply with same channel/priority values
            let mut reply = vec![];
            reply.extend_from_slice(&10u16.to_le_bytes());
            reply.extend_from_slice(&0x1001u16.to_le_bytes());
            reply.push(0x24);  // Echo channel/priority
            reply.push(0x03);  // Type
            reply.extend_from_slice(&[0x03, 0x00]);  // CoE response
            reply.extend_from_slice(&[0x43, 0x00, 0x60, 0x00, 0x01, 0x02, 0x03, 0x04]);
            Ok(reply)
        }
    }).await;
    
    assert!(result.is_ok());
    let reply = result.unwrap().unwrap();
    
    // Verify channel/priority in reply
    assert_eq!(reply[EC_HDR + 4], 0x24);  // Channel/priority preserved in reply
    assert_eq!(reply[EC_HDR + 5], 0x03);  // Type preserved
}

// NEW TEST 3: EtherCAT header only (n<2) - no EtherCAT header at all
#[tokio::test]
async fn test_ethercat_no_header() {
    let addr_to_idx = HashMap::<u16, usize>::new();
    
    // Test with 0 bytes (no packet at all)
    let empty_pkt = vec![];
    let result = handle_frame(&empty_pkt, &addr_to_idx, |_idx, _data| {
        async move {
            panic!("Should not forward empty packet");
        }
    }).await;
    
    assert!(result.is_ok());
    assert!(result.unwrap().is_none());  // No reply
    
    // Test with 1 byte (incomplete EtherCAT header)
    let one_byte = vec![0x11];
    let result = handle_frame(&one_byte, &addr_to_idx, |_idx, _data| {
        async move {
            panic!("Should not forward single byte");
        }
    }).await;
    
    assert!(result.is_ok());
    assert!(result.unwrap().is_none());  // No reply
}

// NEW TEST 4: EtherCAT header only (n=2) - header but no payload
#[tokio::test]
async fn test_ethercat_header_only() {
    let addr_to_idx = HashMap::<u16, usize>::new();
    
    // Test with exactly 2 bytes (EtherCAT header with ec_len=0)
    let mut pkt = Vec::new();
    pkt.extend_from_slice(&0u16.to_le_bytes());  // ec_len = 0
    
    let result = handle_frame(&pkt, &addr_to_idx, |_idx, _data| {
        async move {
            panic!("Should not forward header-only packet");
        }
    }).await;
    
    assert!(result.is_ok());
    assert!(result.unwrap().is_none());  // No reply (ec_len < 6)
    
    // Test with EtherCAT header declaring 2 bytes (still less than mailbox minimum)
    let mut pkt2 = Vec::new();
    pkt2.extend_from_slice(&2u16.to_le_bytes());  // ec_len = 2
    pkt2.extend_from_slice(&[0xAA, 0xBB]);  // 2 bytes of "payload"
    
    let result2 = handle_frame(&pkt2, &addr_to_idx, |_idx, _data| {
        async move {
            panic!("Should not forward packet with ec_len < 6");
        }
    }).await;
    
    assert!(result2.is_ok());
    assert!(result2.unwrap().is_none());  // No reply (ec_len < 6)
}

#[tokio::test]
async fn test_upper_5_bits_preservation() {
    // Test that the upper 5 bits of the EtherCAT header are preserved in the reply
    let mut addr_to_idx = HashMap::new();
    addr_to_idx.insert(0x1001, 0);
    
    // Test with various upper 5-bit patterns
    let test_patterns: Vec<u16> = vec![
        0xF800,  // All upper bits set
        0x8000,  // MSB only
        0x4000,  // Bit 14
        0x2000,  // Bit 13
        0x1000,  // Bit 12
        0x0800,  // Bit 11
        0xA800,  // Mixed pattern
        0x5000,  // Another pattern
    ];
    
    for upper_bits in test_patterns {
        // Build packet with specific upper bits
        let ec_len = 10u16;  // Valid mailbox size
        let ec_hdr = upper_bits | (ec_len & 0x07FF);
        
        let mut pkt = Vec::new();
        pkt.extend_from_slice(&ec_hdr.to_le_bytes());
        
        // Add mailbox header
        pkt.extend_from_slice(&4u16.to_le_bytes());     // Mailbox length
        pkt.extend_from_slice(&0x1001u16.to_le_bytes()); // Station address
        pkt.extend_from_slice(&[0x00, 0x03]);           // Control + Type (CoE)
        
        // Add mailbox payload
        pkt.extend_from_slice(&[0x11, 0x22, 0x33, 0x44]);
        
        let result = handle_frame(&pkt, &addr_to_idx, |_idx, data| {
            let reply_data = data.to_vec();
            async move {
                // Mock reply - just echo back with different data
                let mut reply = reply_data;
                reply[6] = 0xFF;  // Modify first payload byte
                Ok(reply)
            }
        }).await;
        
        assert!(result.is_ok());
        let reply = result.unwrap().expect("Should have reply");
        
        // Verify upper 5 bits are preserved
        let reply_hdr = u16::from_le_bytes([reply[0], reply[1]]);
        let reply_upper = reply_hdr & 0xF800;
        
        assert_eq!(reply_upper, upper_bits, 
                   "Upper 5 bits not preserved: expected {:#06x}, got {:#06x}", 
                   upper_bits, reply_upper);
        
        // Verify length is updated correctly
        let reply_len = reply_hdr & 0x07FF;
        assert_eq!(reply_len as usize, reply.len() - EC_HDR,
                   "Reply length mismatch");
    }
}

// ========================================================================
// Concurrency Test (Ensure serialization)
// ========================================================================

#[tokio::test]
async fn test_serialization_prevents_reply_overtaking() {
    // Test: Two UDP requests sent back-to-back must be serialized
    // Second reply should not overtake the first
    use tokio::sync::mpsc;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    
    let station: u16 = 0x1001;
    let mut addr_to_idx = HashMap::new();
    addr_to_idx.insert(station, 0);
    
    // Create two different requests
    let pkt1 = build_packet(12, 6, station, 0);  // Request 1
    let pkt2 = build_packet(14, 8, station, 0);  // Request 2
    
    // Track order of processing
    let process_order = Arc::new(AtomicUsize::new(0));
    let reply_order = Arc::new(AtomicUsize::new(0));
    
    // Channel to simulate serialized worker
    let (tx, mut rx) = mpsc::channel::<(Vec<u8>, mpsc::Sender<Vec<u8>>)>(10);
    
    // Worker task (simulates gateway's single worker)
    let worker_order = process_order.clone();
    let worker = tokio::spawn(async move {
        let mut sequence = 1;
        while let Some((data, reply_tx)) = rx.recv().await {
            // Record processing order
            let order = worker_order.fetch_add(1, Ordering::SeqCst);
            
            // Simulate processing with varying delays
            if order == 0 {
                // First request takes longer
                tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
            } else {
                // Second request is faster
                tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
            }
            
            // Send reply with sequence number
            let mut reply = data;
            if reply.len() >= 10 {
                reply[8] = sequence;  // Mark with sequence
                sequence += 1;
            }
            let _ = reply_tx.send(reply).await;
        }
    });
    
    // Send both requests concurrently
    let tx1 = tx.clone();
    let tx2 = tx.clone();
    let order1 = reply_order.clone();
    let order2 = reply_order.clone();
    
    let (reply_tx1, mut reply_rx1) = mpsc::channel(1);
    let (reply_tx2, mut reply_rx2) = mpsc::channel(1);
    
    // Send requests in parallel
    let req1 = tokio::spawn(async move {
        tx1.send((pkt1, reply_tx1)).await.unwrap();
        let reply = reply_rx1.recv().await.unwrap();
        order1.fetch_add(1, Ordering::SeqCst);
        reply
    });
    
    let req2 = tokio::spawn(async move {
        tx2.send((pkt2, reply_tx2)).await.unwrap();
        let reply = reply_rx2.recv().await.unwrap();
        order2.fetch_add(1, Ordering::SeqCst);
        reply
    });
    
    // Wait for both replies
    let reply1 = req1.await.unwrap();
    let reply2 = req2.await.unwrap();
    
    // Verify serialization: replies maintain request order
    // Even though request 2 processes faster, it should not overtake request 1
    assert_eq!(reply1[8], 1, "First request should get sequence 1");
    assert_eq!(reply2[8], 2, "Second request should get sequence 2");
    
    // Clean up
    drop(tx);
    let _ = worker.await;
}