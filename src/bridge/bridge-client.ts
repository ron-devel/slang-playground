// Browser-side WebSocket client for the bridge daemon, identifying as a
// UI peer (see bridge/protocol/proto/bridge/v1.proto's PeerRole) — the
// other end of what renderer-android's bridge.rs already speaks. Mirrors
// that side's own shape where it makes sense (auto-connect, retry on
// disconnect, a single always-current connection state rather than an
// event stream), since there's no reason for the two ends of the same
// protocol to be designed differently just because one's Rust and the
// other's TypeScript.
//
// The daemon always runs on the developer's own machine — reachable at
// `ws://127.0.0.1:<port>/ws` either directly (this page and the daemon
// on the same machine) or via bridge-cli's own port-forwarding when the
// page is served from elsewhere. `ws://` (not `wss://`) only works from
// a page itself served over plain `http://` (e.g. `npm run dev`) —
// browsers block a `wss://`-only-capable mixed-content connection from
// an `https://` page, which the deployed GitHub Pages build is. Making
// that work is future work (either a `wss://` daemon, or a local relay
// the deployed page can reach) — for now this targets the local dev
// workflow this project's own `npm run dev` already assumes.

import {
	concatFields,
	decodeEnvelope,
	encodeBytesField,
	encodeMessageField,
	encodeStringField,
	encodeVarintField,
} from "./protobuf";

const DEFAULT_URL = "ws://127.0.0.1:8800/ws";
const RECONNECT_DELAY_MS = 2000;
const PEER_ROLE_UI = 1;

export type BridgeDevice = { sessionId: string; displayName: string };

export type BridgeStatus =
	| { state: "disconnected" }
	| { state: "connecting" }
	// Connected to the daemon; `device` is the currently-attached target
	// (if any) — a UI peer can be connected with no device present.
	| { state: "connected"; device: BridgeDevice | null };

export type ShaderUpdate = {
	computeSpirv: Uint8Array;
	entryPoint: string;
	threadGroupSize: [number, number, number];
	outputTextureBinding: number;
};

function encodeHello(displayName: string): Uint8Array {
	return concatFields(encodeVarintField(1, PEER_ROLE_UI), encodeStringField(2, displayName));
}

function encodeShaderUpdate(update: ShaderUpdate): Uint8Array {
	return concatFields(
		encodeBytesField(1, update.computeSpirv),
		encodeStringField(2, update.entryPoint),
		encodeVarintField(3, update.threadGroupSize[0]),
		encodeVarintField(4, update.threadGroupSize[1]),
		encodeVarintField(5, update.threadGroupSize[2]),
		encodeVarintField(6, update.outputTextureBinding),
	);
}

/// Connects to the bridge daemon and keeps reconnecting for as long as
/// this client is alive, same policy as (and for the same reason as)
/// renderer-android's own `BridgeClient.kt`: the daemon might not be up
/// yet, or might restart, and there's no user-facing action needed to
/// recover from either — it should just quietly work again once it can.
export class BridgeClient {
	private socket: WebSocket | null = null;
	private status: BridgeStatus = { state: "disconnected" };
	private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
	private stopped = false;

	constructor(
		private readonly url: string = DEFAULT_URL,
		private readonly displayName: string = "Slang Playground (web)",
		private readonly onStatusChange?: (status: BridgeStatus) => void,
	) { }

	connect() {
		this.stopped = false;
		this.openSocket();
	}

	disconnect() {
		this.stopped = true;
		if (this.reconnectTimer !== null) {
			clearTimeout(this.reconnectTimer);
			this.reconnectTimer = null;
		}
		this.socket?.close();
		this.socket = null;
	}

	get currentStatus(): BridgeStatus {
		return this.status;
	}

	get connectedDevice(): BridgeDevice | null {
		return this.status.state === "connected" ? this.status.device : null;
	}

	/// No-ops (rather than queuing or erroring) if not currently
	/// connected to a device — matches bridge-core's own "no target
	/// connected, drop it" relay semantics; there's nothing more useful
	/// to do with an update nobody can receive.
	sendShaderUpdate(update: ShaderUpdate) {
		if (this.socket?.readyState !== WebSocket.OPEN) {
			console.info("[bridge] sendShaderUpdate: socket not open (readyState =", this.socket?.readyState, "), dropped");
			return;
		}
		const bytes = encodeMessageField(4, encodeShaderUpdate(update));
		this.socket.send(bytes);
		console.info("[bridge] sendShaderUpdate: sent", bytes.length, "bytes over the socket");
	}

	private openSocket() {
		this.setStatus({ state: "connecting" });

		const socket = new WebSocket(this.url);
		socket.binaryType = "arraybuffer";
		this.socket = socket;

		socket.onopen = () => {
			socket.send(encodeMessageField(1, encodeHello(this.displayName)));
		};

		socket.onmessage = (event) => {
			if (!(event.data instanceof ArrayBuffer)) return;
			const envelope = decodeEnvelope(new Uint8Array(event.data));
			if (envelope.type === "helloAck") {
				this.setStatus({ state: "connected", device: null });
			} else if (envelope.type === "presenceUpdate") {
				this.setStatus({ state: "connected", device: envelope.target });
			}
		};

		socket.onclose = () => {
			if (this.socket !== socket) return; // superseded by a newer connect() already
			this.socket = null;
			this.setStatus({ state: "disconnected" });
			this.scheduleReconnect();
		};

		socket.onerror = () => {
			socket.close();
		};
	}

	private scheduleReconnect() {
		if (this.stopped || this.reconnectTimer !== null) return;
		this.reconnectTimer = setTimeout(() => {
			this.reconnectTimer = null;
			if (!this.stopped) this.openSocket();
		}, RECONNECT_DELAY_MS);
	}

	private setStatus(status: BridgeStatus) {
		this.status = status;
		this.onStatusChange?.(status);
	}
}
