// A minimal, hand-written protobuf wire-format codec for exactly the
// messages the bridge protocol needs from a browser UI peer (see
// bridge/protocol/proto/bridge/v1.proto — the source of truth this must
// stay in sync with by hand, since there's no codegen step wiring the
// two together). Not a general-purpose protobuf library: only the wire
// types these specific messages use (varint, length-delimited) are
// implemented, and only the specific messages a UI peer sends/receives
// are covered — a Target-only message like `ShaderUpdate` doesn't need a
// decoder here, and a Target-only field wouldn't need an encoder.

function encodeVarint(value: number): Uint8Array {
	const bytes: number[] = [];
	let remaining = value >>> 0;
	while (remaining > 0x7f) {
		bytes.push((remaining & 0x7f) | 0x80);
		remaining >>>= 7;
	}
	bytes.push(remaining);
	return new Uint8Array(bytes);
}

function decodeVarint(bytes: Uint8Array, offset: number): [value: number, nextOffset: number] {
	let result = 0;
	let shift = 0;
	let position = offset;
	for (; ;) {
		const byte = bytes[position];
		result |= (byte & 0x7f) << shift;
		position++;
		if ((byte & 0x80) === 0) break;
		shift += 7;
	}
	return [result >>> 0, position];
}

function concatBytes(...chunks: Uint8Array[]): Uint8Array {
	const length = chunks.reduce((sum, chunk) => sum + chunk.length, 0);
	const result = new Uint8Array(length);
	let offset = 0;
	for (const chunk of chunks) {
		result.set(chunk, offset);
		offset += chunk.length;
	}
	return result;
}

const WIRE_TYPE_VARINT = 0;
const WIRE_TYPE_LENGTH_DELIMITED = 2;
const WIRE_TYPE_FIXED32 = 5;

function encodeTag(fieldNumber: number, wireType: number): Uint8Array {
	return encodeVarint((fieldNumber << 3) | wireType);
}

export function encodeVarintField(fieldNumber: number, value: number): Uint8Array {
	return concatBytes(encodeTag(fieldNumber, WIRE_TYPE_VARINT), encodeVarint(value));
}

export function encodeBytesField(fieldNumber: number, value: Uint8Array): Uint8Array {
	return concatBytes(
		encodeTag(fieldNumber, WIRE_TYPE_LENGTH_DELIMITED),
		encodeVarint(value.length),
		value,
	);
}

export function encodeStringField(fieldNumber: number, value: string): Uint8Array {
	return encodeBytesField(fieldNumber, new TextEncoder().encode(value));
}

/// Encodes `message` (already-serialized bytes) as an embedded-message
/// field, e.g. wrapping a `Hello` inside an `Envelope`'s `oneof`.
export function encodeMessageField(fieldNumber: number, message: Uint8Array): Uint8Array {
	return encodeBytesField(fieldNumber, message);
}

export function concatFields(...fields: Uint8Array[]): Uint8Array {
	return concatBytes(...fields);
}

type DecodedField = { fieldNumber: number; wireType: number; value: number | Uint8Array };

function decodeFields(bytes: Uint8Array): DecodedField[] {
	const fields: DecodedField[] = [];
	let offset = 0;
	while (offset < bytes.length) {
		const [tag, afterTag] = decodeVarint(bytes, offset);
		const fieldNumber = tag >>> 3;
		const wireType = tag & 0x7;
		offset = afterTag;
		if (wireType === WIRE_TYPE_VARINT) {
			const [value, afterValue] = decodeVarint(bytes, offset);
			fields.push({ fieldNumber, wireType, value });
			offset = afterValue;
		} else if (wireType === WIRE_TYPE_LENGTH_DELIMITED) {
			const [length, afterLength] = decodeVarint(bytes, offset);
			fields.push({ fieldNumber, wireType, value: bytes.slice(afterLength, afterLength + length) });
			offset = afterLength + length;
		} else if (wireType === WIRE_TYPE_FIXED32) {
			// The only proto3 field this protocol uses that needs this wire
			// type is PerfSample's `gpu_time_ms` (a `float`) — stored as its
			// raw 4 little-endian bytes, decoded by decodeFloatField below.
			fields.push({ fieldNumber, wireType, value: bytes.slice(offset, offset + 4) });
			offset += 4;
		} else {
			// Not a wire type any message in this protocol actually
			// uses — nothing to skip-and-continue correctly (the byte
			// length of a fixed32/fixed64 field is known, but there's no
			// call site that would ever produce one), so bail rather
			// than silently misparse the rest of the message.
			throw new Error(`unsupported protobuf wire type ${wireType}`);
		}
	}
	return fields;
}

function findField(fields: DecodedField[], fieldNumber: number): DecodedField | undefined {
	return fields.find((field) => field.fieldNumber === fieldNumber);
}

function decodeStringField(fields: DecodedField[], fieldNumber: number): string {
	const field = findField(fields, fieldNumber);
	if (!field || !(field.value instanceof Uint8Array)) return "";
	return new TextDecoder().decode(field.value);
}

function decodeVarintField(fields: DecodedField[], fieldNumber: number): number {
	const field = findField(fields, fieldNumber);
	if (!field || typeof field.value !== "number") return 0;
	return field.value;
}

function decodeFloatField(fields: DecodedField[], fieldNumber: number): number {
	const field = findField(fields, fieldNumber);
	if (!field || !(field.value instanceof Uint8Array) || field.value.length !== 4) return 0;
	// Wire-format fixed32 (and thus `float`) is little-endian.
	return new DataView(field.value.buffer, field.value.byteOffset, 4).getFloat32(0, true);
}

export type DecodedTargetInfo = { sessionId: string; displayName: string };

// Field names/shape mirror bridge/protocol/proto/bridge/v1.proto's
// DeviceInfo exactly — see that message's own doc comment for where
// each field comes from (VkPhysicalDeviceProperties vs. android.os.Build
// on the target side).
export type DecodedDeviceInfo = {
	gpuName: string;
	driverVersion: number;
	vendorId: number;
	deviceId: number;
	apiVersion: number;
	androidModel: string;
	androidManufacturer: string;
	androidRelease: string;
	androidSdkInt: number;
	androidFingerprint: string;
};

export type DecodedEnvelope =
	| { type: "helloAck"; sessionId: string }
	| { type: "presenceUpdate"; target: DecodedTargetInfo | null }
	| { type: "deviceInfo"; info: DecodedDeviceInfo }
	| { type: "perfSample"; frameId: number; gpuTimeMs: number }
	| { type: "unknown" };

/// Decodes an `Envelope` as far as `HelloAck`/`PresenceUpdate`/
/// `DeviceInfo`/`PerfSample` — the variants a UI peer can receive today
/// (see this file's own doc comment; bridge-core doesn't relay
/// `DeviceInfo`/`PerfSample` to UI peers yet, but decoding them here
/// ahead of that costs nothing). Anything else (a `Hello`/`ShaderUpdate`
/// a buggy or future daemon somehow echoed back) decodes as `"unknown"`
/// rather than throwing, matching the same forward-compatible leniency
/// bridge-core itself extends to unrecognized frames.
export function decodeEnvelope(bytes: Uint8Array): DecodedEnvelope {
	const fields = decodeFields(bytes);

	const helloAck = findField(fields, 2);
	if (helloAck && helloAck.value instanceof Uint8Array) {
		const inner = decodeFields(helloAck.value);
		return { type: "helloAck", sessionId: decodeStringField(inner, 1) };
	}

	const presenceUpdate = findField(fields, 3);
	if (presenceUpdate && presenceUpdate.value instanceof Uint8Array) {
		const inner = decodeFields(presenceUpdate.value);
		const targetInfo = findField(inner, 1);
		if (!targetInfo || !(targetInfo.value instanceof Uint8Array)) {
			return { type: "presenceUpdate", target: null };
		}
		const targetFields = decodeFields(targetInfo.value);
		return {
			type: "presenceUpdate",
			target: {
				sessionId: decodeStringField(targetFields, 1),
				displayName: decodeStringField(targetFields, 2),
			},
		};
	}

	const deviceInfo = findField(fields, 5);
	if (deviceInfo && deviceInfo.value instanceof Uint8Array) {
		const inner = decodeFields(deviceInfo.value);
		return {
			type: "deviceInfo",
			info: {
				gpuName: decodeStringField(inner, 1),
				driverVersion: decodeVarintField(inner, 2),
				vendorId: decodeVarintField(inner, 3),
				deviceId: decodeVarintField(inner, 4),
				apiVersion: decodeVarintField(inner, 5),
				androidModel: decodeStringField(inner, 6),
				androidManufacturer: decodeStringField(inner, 7),
				androidRelease: decodeStringField(inner, 8),
				androidSdkInt: decodeVarintField(inner, 9),
				androidFingerprint: decodeStringField(inner, 10),
			},
		};
	}

	const perfSample = findField(fields, 6);
	if (perfSample && perfSample.value instanceof Uint8Array) {
		const inner = decodeFields(perfSample.value);
		return {
			type: "perfSample",
			frameId: decodeVarintField(inner, 1),
			gpuTimeMs: decodeFloatField(inner, 2),
		};
	}

	return { type: "unknown" };
}
