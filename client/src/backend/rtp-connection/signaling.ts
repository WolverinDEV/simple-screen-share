import { C2SMessage, C2SRequest, S2CMessage, S2CNotify, S2CResponse } from "@sss-definitions/rtp-messages";
import { RtpEvents } from ".";
import { EventEmitter } from "../../utils/ee2";

export type SignallingState = {
    status: "unconnected",
} | {
    status: "connecting",
} | {
    status: "connected"
} | {
    status: "failed",
    reason: string
} | {
    status: "disconnected",
    reason: string
};

type PendingRequest = {
    timeout: any,
    resolve: (answer: S2CResponse) => void,
};

type NotifyPayload<K extends S2CNotify["type"]> = Extract<S2CNotify, { type: K }> extends { payload: infer U } ? U : never;
type NotifyHandler<K extends S2CNotify["type"]> = (payload: NotifyPayload<K>, type: K) => void;
type NotifyHandlers = {
    [K in S2CNotify["type"]]?: NotifyHandler<K>[]
};


const kRequestErrorConnectionClosed: S2CResponse = { type: "ConnectionClosed" };
const kRequestErrorRequestTimeout: S2CResponse = { type: "RequestTimeout" };
export class SignallingConnection {
    private readonly events: EventEmitter<RtpEvents>;
    private readonly serverUrl: string;

    private socket: WebSocket | null;
    private requestIdIndex: number;
    private requests: Record<number, PendingRequest>;
    private notify: NotifyHandlers;

    public state: SignallingState;

    constructor(events: EventEmitter<RtpEvents>, serverUrl: string) {
        this.events = events;
        this.serverUrl = serverUrl;

        this.state = { status: "unconnected" };
        this.requestIdIndex = 0;
        this.socket = null;
        this.requests = {};
        this.notify = {};
    }

    private updateState(newState: SignallingState) {
        const oldState = this.state;
        this.state = newState;

        this.events.emit("signalling.state_changed", newState, oldState);
    }

    public connect() {
        if(this.state.status !== "unconnected") {
            throw new Error("expected unconnected state");
        }

        this.openSocket();
        this.updateState({ status: "connecting" });
    }

    public close() {
        this.closeSocket();
        this.updateState({ status: "unconnected" });
    }

    private openSocket() {
        if(this.socket !== null) {
            throw new Error("socket already exists");
        }

        const socket = this.socket = new WebSocket(this.serverUrl);
        this.socket.onopen = () => this.socket === socket && this.onSocketConnected();
        this.socket.onclose = e => this.socket === socket && this.onSocketClosed(e);
        this.socket.onerror = () => this.socket === socket && this.onSocketError();
        this.socket.onmessage = e => this.socket === socket && this.onSocketMessage(e);
    }

    private closeSocket() {
        if(!this.socket) {
            return;
        }

        this.socket.onopen = undefined;
        this.socket.onclose = undefined;
        this.socket.onerror = undefined;
        this.socket.onmessage = undefined;
        if(this.socket.readyState === WebSocket.OPEN) {
            this.socket.close();
        }
        this.socket = null;
    }

    private abortPendingRequests() {
        for(const requestId of Object.keys(this.requests)) {
            const request = this.requests[parseInt(requestId)];
            delete this.requests[requestId];
            clearTimeout(request.timeout);
            request.resolve(kRequestErrorConnectionClosed);
        }
    }

    public async sendRequest<R extends C2SRequest["type"]>(request: R, payload: Omit<C2SRequest & { type: R }, "type">) : Promise<S2CResponse> {
        if(this.socket?.readyState !== WebSocket.OPEN) {
            return kRequestErrorConnectionClosed;
        }

        const requestId = (++this.requestIdIndex) & 0xFFFF;
        this.socket.send(JSON.stringify({
            type: "Request",
            payload: {
                request_id: requestId,
                request: {
                    type: request,
                    ...payload
                } as any
            }
        } as C2SMessage));

        const result = await new Promise<S2CResponse>(resolve => {
            const timeout = setTimeout(() => resolve(kRequestErrorRequestTimeout), 15_000);
            this.requests[requestId] = {
                resolve,
                timeout
            };
        });

        if(requestId in this.requests) {
            clearTimeout(this.requests[requestId].timeout);
            delete this.requests[requestId];
        }
        return result;
    }
    
    private async onSocketConnected() {
        this.updateState({ status: "connected" });
    }

    private onSocketClosed(event: CloseEvent) {
        this.updateState({ status: "disconnected", reason: event.reason ? `${event.code} / ${event.reason}` : `${event.code ?? 0}` });
        this.closeSocket();
        this.abortPendingRequests();
    }

    private onSocketError() {
        this.updateState({ status: "failed", reason: "socket io failure" });
        this.closeSocket();
        this.abortPendingRequests();
    }

    private onSocketMessage(event: MessageEvent) {
        if(typeof event.data !== "string") {
            console.warn("Received non text message.");
            return;
        }

        let message: S2CMessage;
        try {
            message = JSON.parse(event.data) as S2CMessage;
        } catch {
            console.warn("Failed to parse message from server.");
            return;
        }

        try {
            this.handleMessage(message);
        } catch(error) {
            console.error(`Failed to handle server message: %o`, error);
        }
    }

    private handleMessage(message: S2CMessage) {
        switch(message.type) {
            case "Response":
                const [ requestId, response ] = message.payload;
                const request = this.requests[requestId];
                delete this.requests[requestId];
                if(!request) {
                    console.warn("Received response for unknown request %d", requestId);
                    return;
                }

                clearTimeout(request.timeout);
                request.resolve(response);
                break;

            case "Notify":
                const handlers = this.notify[message.payload.type];
                if(!Array.isArray(handlers) || handlers.length === 0) {
                    console.warn("Received notify without a handler: %s", message.payload.type);
                    return;
                }

                for(const handler of handlers) {
                    try {
                        // @ts-ignore
                        handler(message.payload.payload, message.payload.type);
                    } catch(error) {
                        console.error("Notify handler exception cought: %o", error);
                    }
                }
                break; 
        }
    }

    public registerNotifyHandler<K extends S2CNotify["type"]>(notify: K, handler: NotifyHandler<K>) {
        const handlers = this.notify[notify] ?? (this.notify[notify] = []);
        handlers.push(handler);
    }
}