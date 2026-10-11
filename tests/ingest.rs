use cranelift_isr::ingest::{BinaryInput, Selection};

// A minimal linked x86-64 Mach-O image with a normal __TEXT/__text mapping
// and LC_MAIN. The entry field is a file offset, deliberately far from its VA.
fn macho(entry_file_offset: u64) -> Vec<u8> {
    let mut file = Vec::new();
    for value in [0xfeedfacfu32, 0x01000007, 3, 2, 2, 176, 0, 0] {
        file.extend_from_slice(&value.to_le_bytes());
    }
    for value in [0x19u32, 152] {
        file.extend_from_slice(&value.to_le_bytes());
    }
    file.extend_from_slice(b"__TEXT\0\0\0\0\0\0\0\0\0\0");
    for value in [0x1_0000_0000u64, 0x1000, 0, 0x205] {
        file.extend_from_slice(&value.to_le_bytes());
    }
    for value in [5u32, 5, 1, 0] {
        file.extend_from_slice(&value.to_le_bytes());
    }
    file.extend_from_slice(b"__text\0\0\0\0\0\0\0\0\0\0");
    file.extend_from_slice(b"__TEXT\0\0\0\0\0\0\0\0\0\0");
    for value in [0x1_0000_0200u64, 5] {
        file.extend_from_slice(&value.to_le_bytes());
    }
    for value in [0x200u32, 4, 0, 0, 0x80000400, 0, 0, 0] {
        file.extend_from_slice(&value.to_le_bytes());
    }
    for value in [0x80000028u32, 24] {
        file.extend_from_slice(&value.to_le_bytes());
    }
    file.extend_from_slice(&entry_file_offset.to_le_bytes());
    file.extend_from_slice(&0u64.to_le_bytes());
    assert_eq!(file.len(), 32 + 176);
    file.resize(0x200, 0);
    file.extend_from_slice(&[0x48, 0x83, 0xc0, 7, 0xc3]);
    file
}

#[test]
fn macho_entry_offset_is_resolved_without_changing_explicit_virtual_addresses() {
    let file = macho(0x200);
    let entry = BinaryInput::parse(&file, &Selection::Entry).unwrap();
    assert_eq!(entry.address, 0x1_0000_0200);
    assert_eq!(entry.bytes, [0x48, 0x83, 0xc0, 7, 0xc3]);
    assert_eq!(entry.metadata["entry_file_offset"], 0x200);
    assert_eq!(entry.metadata["file_offset"], 0x200);
    let explicit = BinaryInput::parse(&file, &Selection::Address(entry.address)).unwrap();
    assert_eq!(explicit.address, entry.address);
    assert_eq!(explicit.bytes, entry.bytes);
    assert!(BinaryInput::parse(&file, &Selection::Address(0x200)).is_err());
    for offset in [0, 0x1ff, 0x205, u64::MAX] {
        assert!(BinaryInput::parse(&macho(offset), &Selection::Entry).is_err());
    }
}
