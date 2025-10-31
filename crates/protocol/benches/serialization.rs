//! Benchmarks for protocol serialization
//!
//! Measures encoding/decoding performance for different message types:
//! - Simple messages (Ping, Pong)
//! - Device discovery responses
//! - USB transfers (control, interrupt, bulk)
//! - Framed messages

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use protocol::{
    CURRENT_VERSION, DeviceHandle, DeviceId, DeviceInfo, DeviceSpeed, Message, MessagePayload,
    RequestId, TransferType, UsbRequest, decode_framed, decode_message, encode_framed,
    encode_message,
};

fn benchmark_simple_messages(c: &mut Criterion) {
    let mut group = c.benchmark_group("simple_messages");

    let ping = Message {
        version: CURRENT_VERSION,
        payload: MessagePayload::Ping,
    };

    group.bench_function("encode_ping", |b| {
        b.iter(|| encode_message(black_box(&ping)))
    });

    let ping_bytes = encode_message(&ping).unwrap();
    group.bench_function("decode_ping", |b| {
        b.iter(|| decode_message(black_box(&ping_bytes)))
    });

    group.bench_function("roundtrip_ping", |b| {
        b.iter(|| {
            let bytes = encode_message(black_box(&ping)).unwrap();
            decode_message(&bytes).unwrap()
        })
    });

    group.finish();
}

fn benchmark_device_list(c: &mut Criterion) {
    let mut group = c.benchmark_group("device_list");

    // Create a realistic device list
    let devices: Vec<DeviceInfo> = (1..=10)
        .map(|i| DeviceInfo {
            id: DeviceId(i),
            vendor_id: 0x1234,
            product_id: 0x5678 + i as u16,
            bus_number: 1,
            device_address: i as u8,
            manufacturer: Some("Test Manufacturer".to_string()),
            product: Some(format!("Test Device {}", i)),
            serial_number: Some(format!("SN{:08}", i)),
            class: 0x08,
            subclass: 0x06,
            protocol: 0x50,
            speed: DeviceSpeed::High,
            num_configurations: 1,
        })
        .collect();

    let msg = Message {
        version: CURRENT_VERSION,
        payload: MessagePayload::ListDevicesResponse {
            devices: devices.clone(),
        },
    };

    group.bench_function("encode_10_devices", |b| {
        b.iter(|| encode_message(black_box(&msg)))
    });

    let bytes = encode_message(&msg).unwrap();
    group.bench_function("decode_10_devices", |b| {
        b.iter(|| decode_message(black_box(&bytes)))
    });

    group.finish();
}

fn benchmark_usb_transfers(c: &mut Criterion) {
    let mut group = c.benchmark_group("usb_transfers");

    // Control transfer (64 bytes)
    let control_request = UsbRequest {
        id: RequestId(1),
        handle: DeviceHandle(1),
        transfer: TransferType::Control {
            request_type: 0x80,
            request: 0x06,
            value: 0x0100,
            index: 0,
            data: vec![0u8; 64],
        },
    };

    let control_msg = Message {
        version: CURRENT_VERSION,
        payload: MessagePayload::SubmitTransfer {
            request: control_request,
        },
    };

    group.throughput(Throughput::Bytes(64));
    group.bench_function("control_64bytes", |b| {
        b.iter(|| encode_message(black_box(&control_msg)))
    });

    // Bulk transfer (4KB)
    let bulk_request = UsbRequest {
        id: RequestId(2),
        handle: DeviceHandle(1),
        transfer: TransferType::Bulk {
            endpoint: 0x81,
            data: vec![0xAB; 4096],
            timeout_ms: 5000,
        },
    };

    let bulk_msg = Message {
        version: CURRENT_VERSION,
        payload: MessagePayload::SubmitTransfer {
            request: bulk_request,
        },
    };

    group.throughput(Throughput::Bytes(4096));
    group.bench_function("bulk_4kb", |b| {
        b.iter(|| encode_message(black_box(&bulk_msg)))
    });

    // Interrupt transfer (8 bytes - typical HID)
    let interrupt_request = UsbRequest {
        id: RequestId(3),
        handle: DeviceHandle(1),
        transfer: TransferType::Interrupt {
            endpoint: 0x81,
            data: vec![0u8; 8],
            timeout_ms: 1000,
        },
    };

    let interrupt_msg = Message {
        version: CURRENT_VERSION,
        payload: MessagePayload::SubmitTransfer {
            request: interrupt_request,
        },
    };

    group.throughput(Throughput::Bytes(8));
    group.bench_function("interrupt_8bytes", |b| {
        b.iter(|| encode_message(black_box(&interrupt_msg)))
    });

    group.finish();
}

fn benchmark_framing(c: &mut Criterion) {
    let mut group = c.benchmark_group("framing");

    let msg = Message {
        version: CURRENT_VERSION,
        payload: MessagePayload::Ping,
    };

    group.bench_function("encode_framed_ping", |b| {
        b.iter(|| encode_framed(black_box(&msg)))
    });

    let framed = encode_framed(&msg).unwrap();
    group.bench_function("decode_framed_ping", |b| {
        b.iter(|| decode_framed(black_box(&framed)))
    });

    // Large framed message
    let devices: Vec<DeviceInfo> = (1..=100)
        .map(|i| DeviceInfo {
            id: DeviceId(i),
            vendor_id: 0x1234,
            product_id: 0x5678,
            bus_number: 1,
            device_address: i as u8,
            manufacturer: Some("A".repeat(50)),
            product: Some("B".repeat(50)),
            serial_number: Some("C".repeat(30)),
            class: 0x08,
            subclass: 0x06,
            protocol: 0x50,
            speed: DeviceSpeed::High,
            num_configurations: 1,
        })
        .collect();

    let large_msg = Message {
        version: CURRENT_VERSION,
        payload: MessagePayload::ListDevicesResponse { devices },
    };

    group.bench_function("encode_framed_100_devices", |b| {
        b.iter(|| encode_framed(black_box(&large_msg)))
    });

    let large_framed = encode_framed(&large_msg).unwrap();
    group.bench_function("decode_framed_100_devices", |b| {
        b.iter(|| decode_framed(black_box(&large_framed)))
    });

    group.finish();
}

fn benchmark_bulk_sizes(c: &mut Criterion) {
    let mut group = c.benchmark_group("bulk_transfer_sizes");

    for size in [64, 512, 1024, 4096, 16384, 65536].iter() {
        let request = UsbRequest {
            id: RequestId(1),
            handle: DeviceHandle(1),
            transfer: TransferType::Bulk {
                endpoint: 0x81,
                data: vec![0xAB; *size],
                timeout_ms: 5000,
            },
        };

        let msg = Message {
            version: CURRENT_VERSION,
            payload: MessagePayload::SubmitTransfer { request },
        };

        group.throughput(Throughput::Bytes(*size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, _| {
            b.iter(|| encode_message(black_box(&msg)))
        });
    }

    group.finish();
}

criterion_group!(
    benches,
    benchmark_simple_messages,
    benchmark_device_list,
    benchmark_usb_transfers,
    benchmark_framing,
    benchmark_bulk_sizes
);
criterion_main!(benches);
