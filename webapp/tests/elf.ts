export function elf(code: Buffer, size = 256 + code.length) {
  const image = Buffer.alloc(size);
  Buffer.from([0x7f, 0x45, 0x4c, 0x46, 2, 1, 1]).copy(image);
  image.writeUInt16LE(2, 16);
  image.writeUInt16LE(62, 18);
  image.writeUInt32LE(1, 20);
  image.writeBigUInt64LE(0x20100n, 24);
  image.writeBigUInt64LE(64n, 32);
  for (const [at, value] of [
    [52, 64],
    [54, 56],
    [56, 1],
    [58, 64],
  ])
    image.writeUInt16LE(value, at);
  image.writeUInt32LE(1, 64);
  image.writeUInt32LE(5, 68);
  image.writeBigUInt64LE(0x20000n, 80);
  image.writeBigUInt64LE(0x20000n, 88);
  image.writeBigUInt64LE(BigInt(size), 96);
  image.writeBigUInt64LE(BigInt(size), 104);
  image.writeBigUInt64LE(4096n, 112);
  code.copy(image, 256);
  return image;
}
