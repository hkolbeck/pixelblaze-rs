use std::collections::HashSet;
use std::io::{Cursor, Write};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::time::SystemTime;
use std::io::Result;

const SENSOR_DATA_BYTES: usize = 32 * 2 + //Frequency buckets
    2 + // Energy average
    2 + // maxFreqMagnitude
    2 + // maxFreq
    3 * 2 + // Accelerometer
    2 + // light
    5 * 2; // Analog inputs

const DISC_HEADER_BYTES: usize = 4 + // Packet type
    4 + // Sender ID
    4 + // Sender TS millis
    1 + // Expansion type
    3; // Padding

const FRAME_BYTES: usize = DISC_HEADER_BYTES + SENSOR_DATA_BYTES;


pub struct AudioData {
    pub freq_buckets: [u16; 32],
    pub energy_avg: u16,
    pub max_freq_magnitude: u16,
    pub max_freq: u16,
}

pub struct SensorClient {
    sender_id: [u8; 4],
    targets: HashSet<SocketAddr>,
    frame_type: [u8; 4],
}

impl SensorClient {
    pub fn new(sender_id: u32) -> SensorClient {
        SensorClient {
            sender_id: sender_id.to_le_bytes(),
            targets: HashSet::new(),
            frame_type: 50_i32.to_le_bytes(),
        }
    }

    pub fn add_target(&mut self, addr: SocketAddr) {
        self.targets.insert(addr);
    }

    pub fn remove_target(&mut self, addr: &SocketAddr) {
        self.targets.remove(addr);
    }

    pub fn send_frame(
        &self,
        audio: &AudioData,
        accel: &[i16; 3],
        light: u16,
        analog: &[u16; 5],
    ) -> Result<()> {
        let ts_millis: u32 = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|dur| dur.as_millis().try_into().unwrap_or(u32::MAX))
            .unwrap_or(0);

        let mut cursor = Cursor::new([0; FRAME_BYTES]);

        // Write header
        cursor.write_all(&self.frame_type)?;
        cursor.write_all(&self.sender_id)?;
        cursor.write_all(&ts_millis.to_le_bytes())?;
        cursor.write_all(&[1_u8])?; // It's an SB10 sensor board
        cursor.set_position(cursor.position() + 3);

        // Audio
        for bucket in &audio.freq_buckets {
            cursor.write_all(&bucket.to_le_bytes())?;
        }
        cursor.write_all(&audio.energy_avg.to_le_bytes())?;
        cursor.write_all(&audio.max_freq_magnitude.to_le_bytes())?;
        cursor.write_all(&audio.max_freq.to_le_bytes())?;

        // Other sensor data
        for axis in accel {
            cursor.write_all(&axis.to_le_bytes())?;
        }
        cursor.write_all(&light.to_le_bytes())?;
        for input in analog {
            cursor.write_all(&input.to_le_bytes())?;
        }

        assert_eq!(cursor.position(), FRAME_BYTES as u64);

        let frame = cursor.into_inner();
        let socket = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))?;

        for target in &self.targets {
            socket.connect(target)?;
            socket.send(&frame)?;
        }

        Ok(())
    }
}