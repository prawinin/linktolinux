// Standalone QR code matrix generator (ISO/IEC 18004:2015)
// Byte-mode encoding for Version 1 through 6 with Error Correction Level L, M, Q, H.

export type EcLevel = "L" | "M" | "Q" | "H";

interface VersionInfo {
  version: number;
  totalCodewords: number;
  ecCodewords: Record<EcLevel, number>;
  blocks: Record<EcLevel, [number, number][]>; // [numBlocks, dataCodewordsPerBlock]
  alignments: number[];
}

const VERSIONS: VersionInfo[] = [
  {
    version: 1,
    totalCodewords: 26,
    ecCodewords: { L: 7, M: 10, Q: 13, H: 17 },
    blocks: { L: [[1, 19]], M: [[1, 16]], Q: [[1, 13]], H: [[1, 9]] },
    alignments: [],
  },
  {
    version: 2,
    totalCodewords: 44,
    ecCodewords: { L: 10, M: 16, Q: 22, H: 28 },
    blocks: { L: [[1, 34]], M: [[1, 28]], Q: [[1, 22]], H: [[1, 16]] },
    alignments: [6, 18],
  },
  {
    version: 3,
    totalCodewords: 70,
    ecCodewords: { L: 15, M: 26, Q: 36, H: 44 },
    blocks: { L: [[1, 55]], M: [[1, 44]], Q: [[2, 17]], H: [[2, 13]] },
    alignments: [6, 22],
  },
  {
    version: 4,
    totalCodewords: 100,
    ecCodewords: { L: 20, M: 36, Q: 52, H: 64 },
    blocks: { L: [[1, 80]], M: [[2, 32]], Q: [[2, 24]], H: [[4, 9]] },
    alignments: [6, 26],
  },
  {
    version: 5,
    totalCodewords: 134,
    ecCodewords: { L: 26, M: 48, Q: 72, H: 88 },
    blocks: { L: [[1, 108]], M: [[2, 43]], Q: [[2, 15], [2, 16]], H: [[2, 11], [2, 12]] },
    alignments: [6, 30],
  },
  {
    version: 6,
    totalCodewords: 172,
    ecCodewords: { L: 36, M: 64, Q: 96, H: 112 },
    blocks: { L: [[2, 68]], M: [[4, 27]], Q: [[4, 19]], H: [[4, 15]] },
    alignments: [6, 34],
  },
];

const GF_EXP = new Uint8Array(512);
const GF_LOG = new Uint8Array(256);
(function initGF() {
  let x = 1;
  for (let i = 0; i < 255; i++) {
    GF_EXP[i] = x;
    GF_EXP[i + 255] = x;
    GF_LOG[x] = i;
    x <<= 1;
    if (x >= 256) x ^= 0x11d;
  }
})();

function gfMul(x: number, y: number): number {
  if (x === 0 || y === 0) return 0;
  const logX = GF_LOG[x] ?? 0;
  const logY = GF_LOG[y] ?? 0;
  return GF_EXP[logX + logY] ?? 0;
}

function rsGenerator(numEc: number): Uint8Array {
  let poly = new Uint8Array([1]);
  for (let i = 0; i < numEc; i++) {
    const next = new Uint8Array(poly.length + 1);
    const expI = GF_EXP[i] ?? 0;
    for (let j = 0; j < poly.length; j++) {
      const p = poly[j] ?? 0;
      next[j] = (next[j] ?? 0) ^ p;
      next[j + 1] = (next[j + 1] ?? 0) ^ gfMul(p, expI);
    }
    poly = next;
  }
  return poly;
}

function rsEncode(data: Uint8Array, numEc: number): Uint8Array {
  const gen = rsGenerator(numEc);
  const remainder = new Uint8Array(numEc);
  for (let i = 0; i < data.length; i++) {
    const d = data[i] ?? 0;
    const rem0 = remainder[0] ?? 0;
    const factor = d ^ rem0;
    for (let j = 0; j < numEc - 1; j++) {
      const nextRem = remainder[j + 1] ?? 0;
      const genCoeff = gen[j + 1] ?? 0;
      remainder[j] = nextRem ^ gfMul(genCoeff, factor);
    }
    const genLast = gen[numEc] ?? 0;
    remainder[numEc - 1] = gfMul(genLast, factor);
  }
  return remainder;
}

// Format info BCH(15,5) code table: Mask pattern 0 (row + col) % 2 === 0
const FORMAT_INFO: Record<EcLevel, number> = {
  L: 0x77c4,
  M: 0x5412,
  Q: 0x355f,
  H: 0x1689,
};

export function generateQrMatrix(text: string, ecLevel: EcLevel = "M"): boolean[][] {
  const bytes = new TextEncoder().encode(text);
  let targetVersion: VersionInfo = VERSIONS[VERSIONS.length - 1] as VersionInfo;
  for (const v of VERSIONS) {
    const dataCapacity = v.totalCodewords - v.ecCodewords[ecLevel];
    const neededBits = 4 + 8 + bytes.length * 8;
    const neededBytes = Math.ceil(neededBits / 8);
    if (neededBytes <= dataCapacity) {
      targetVersion = v;
      break;
    }
  }

  const dataCapacity = targetVersion.totalCodewords - targetVersion.ecCodewords[ecLevel];

  // 1. Bit buffer
  const bitBuf: number[] = [];
  function pushBits(val: number, len: number) {
    for (let i = len - 1; i >= 0; i--) {
      bitBuf.push((val >>> i) & 1);
    }
  }

  // Byte mode indicator: 0100
  pushBits(0b0100, 4);
  // Character count indicator (8 bits for V1-9)
  pushBits(bytes.length, 8);
  for (const b of bytes) {
    pushBits(b, 8);
  }
  // Terminator
  const remBits = dataCapacity * 8 - bitBuf.length;
  pushBits(0, Math.min(4, remBits));
  // Byte align
  while (bitBuf.length % 8 !== 0) {
    bitBuf.push(0);
  }

  // Convert bits to codewords
  const dataCodewords = new Uint8Array(dataCapacity);
  for (let i = 0; i < bitBuf.length / 8; i++) {
    let byteVal = 0;
    for (let j = 0; j < 8; j++) {
      const bit = bitBuf[i * 8 + j] ?? 0;
      byteVal = (byteVal << 1) | bit;
    }
    dataCodewords[i] = byteVal;
  }
  // Pad bytes alternating 0xEC and 0x11
  let padToggle = 0xec;
  for (let i = Math.floor(bitBuf.length / 8); i < dataCapacity; i++) {
    dataCodewords[i] = padToggle;
    padToggle = padToggle === 0xec ? 0x11 : 0xec;
  }

  // 2. Block partition and Reed-Solomon EC
  const blocksSpec = targetVersion.blocks[ecLevel];
  const numBlocks = blocksSpec.reduce((acc, curr) => acc + curr[0], 0);
  const ecPerBlock = Math.floor(targetVersion.ecCodewords[ecLevel] / numBlocks);

  const dataBlocks: Uint8Array[] = [];
  const ecBlocks: Uint8Array[] = [];

  let dataOffset = 0;
  for (const [count, dcCount] of blocksSpec) {
    for (let b = 0; b < count; b++) {
      const blockData = dataCodewords.slice(dataOffset, dataOffset + dcCount);
      dataBlocks.push(blockData);
      ecBlocks.push(rsEncode(blockData, ecPerBlock));
      dataOffset += dcCount;
    }
  }

  // Interleave data codewords
  const finalCodewords: number[] = [];
  const maxDataLen = Math.max(...dataBlocks.map((b) => b.length));
  for (let i = 0; i < maxDataLen; i++) {
    for (const b of dataBlocks) {
      if (i < b.length) {
        finalCodewords.push(b[i] ?? 0);
      }
    }
  }
  // Interleave EC codewords
  for (let i = 0; i < ecPerBlock; i++) {
    for (const b of ecBlocks) {
      if (i < b.length) {
        finalCodewords.push(b[i] ?? 0);
      }
    }
  }

  // 3. Matrix construction
  const size = 17 + 4 * targetVersion.version;
  const matrix: boolean[][] = Array.from({ length: size }, () => Array(size).fill(false));
  const isFunction: boolean[][] = Array.from({ length: size }, () => Array(size).fill(false));

  function setModule(r: number, c: number, val: boolean, isFunc = true) {
    if (r >= 0 && r < size && c >= 0 && c < size) {
      const row = matrix[r];
      if (row) row[c] = val;
      if (isFunc) {
        const funcRow = isFunction[r];
        if (funcRow) funcRow[c] = true;
      }
    }
  }

  // Finder patterns
  function drawFinder(r: number, c: number) {
    for (let dr = -1; dr <= 7; dr++) {
      for (let dc = -1; dc <= 7; dc++) {
        const row = r + dr;
        const col = c + dc;
        if (row < 0 || row >= size || col < 0 || col >= size) continue;
        const inOuter = dr >= 0 && dr <= 6 && dc >= 0 && dc <= 6;
        const isBorder = dr === 0 || dr === 6 || dc === 0 || dc === 6;
        const inCenter = dr >= 2 && dr <= 4 && dc >= 2 && dc <= 4;
        setModule(row, col, inOuter && (isBorder || inCenter), true);
      }
    }
  }

  drawFinder(0, 0);
  drawFinder(0, size - 7);
  drawFinder(size - 7, 0);

  // Timing patterns
  for (let i = 8; i < size - 8; i++) {
    const val = i % 2 === 0;
    setModule(6, i, val, true);
    setModule(i, 6, val, true);
  }

  // Alignment patterns
  const alignments = targetVersion.alignments;
  for (const r of alignments) {
    for (const c of alignments) {
      const lastAlign = alignments[alignments.length - 1];
      if (
        (r === 6 && c === 6) ||
        (r === 6 && c === lastAlign) ||
        (r === lastAlign && c === 6)
      ) {
        continue;
      }
      for (let dr = -2; dr <= 2; dr++) {
        for (let dc = -2; dc <= 2; dc++) {
          const isBorder = Math.abs(dr) === 2 || Math.abs(dc) === 2;
          const isCenter = dr === 0 && dc === 0;
          setModule(r + dr, c + dc, isBorder || isCenter, true);
        }
      }
    }
  }

  // Dark module
  setModule(size - 8, 8, true, true);

  // Format info reserve
  for (let i = 0; i < 9; i++) {
    const row8 = isFunction[8];
    const rowI = isFunction[i];
    if (row8) row8[i] = true;
    if (rowI) rowI[8] = true;
  }
  for (let i = 0; i < 8; i++) {
    const row8 = isFunction[8];
    const rowRev = isFunction[size - 1 - i];
    if (row8) row8[size - 1 - i] = true;
    if (rowRev) rowRev[8] = true;
  }

  // 4. Place data bits
  let cwIdx = 0;
  let bitIdx = 7;
  let upwards = true;

  for (let c = size - 1; c > 0; c -= 2) {
    if (c === 6) c--; // Skip vertical timing column
    const rows = upwards
      ? Array.from({ length: size }, (_, i) => size - 1 - i)
      : Array.from({ length: size }, (_, i) => i);

    for (const r of rows) {
      for (const col of [c, c - 1]) {
        const funcRow = isFunction[r];
        if (funcRow && !funcRow[col]) {
          let bit = 0;
          if (cwIdx < finalCodewords.length) {
            const word = finalCodewords[cwIdx] ?? 0;
            bit = (word >>> bitIdx) & 1;
            bitIdx--;
            if (bitIdx < 0) {
              bitIdx = 7;
              cwIdx++;
            }
          }
          // Apply mask 0: (row + col) % 2 === 0
          const mask = (r + col) % 2 === 0;
          const matrixRow = matrix[r];
          if (matrixRow) {
            matrixRow[col] = (bit === 1) !== mask;
          }
        }
      }
    }
    upwards = !upwards;
  }

  // Format info placement
  const formatWord = FORMAT_INFO[ecLevel];
  for (let i = 0; i < 15; i++) {
    const bit = ((formatWord >>> i) & 1) === 1;
    // Format bits use column 8 first; data placement is row-major.
    if (i < 6) setModule(i, 8, bit, true);
    else if (i === 6) setModule(7, 8, bit, true);
    else if (i === 7) setModule(8, 8, bit, true);
    else if (i === 8) setModule(8, 7, bit, true);
    else setModule(8, 14 - i, bit, true);

    if (i < 8) setModule(8, size - 1 - i, bit, true);
    else setModule(size - 15 + i, 8, bit, true);
  }

  return matrix;
}

export function generateQrSvgPath(matrix: boolean[][]): { path: string; size: number } {
  const size = matrix.length;
  let path = "";
  for (let r = 0; r < size; r++) {
    const row = matrix[r];
    if (!row) continue;
    for (let c = 0; c < size; c++) {
      if (row[c]) {
        path += `M${c},${r}h1v1h-1z `;
      }
    }
  }
  return { path, size };
}
