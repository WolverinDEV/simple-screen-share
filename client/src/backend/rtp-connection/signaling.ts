import { C2SMessage, C2SRequest, IceCandidate, S2CMessage, S2CNotify, S2CResponse } from "@sss-definitions/rtp-messages";
import { Observable } from "../../utils/observable";
import { VirtualCamera } from "../../utils/virtual-camera";
import { RtcConnection, RtpSignalingConnection } from "./rtc";

type ConnectionState = {
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
type NotifyHandler = {
    [K in S2CNotify["type"]]?: (payload: NotifyPayload<K>, type: K) => void
};


const kRequestErrorConnectionClosed: S2CResponse = { type: "ConnectionClosed" };
const kRequestErrorRequestTimeout: S2CResponse = { type: "RequestTimeout" };
export class RtpServerConnection {
    private readonly serverUrl: string;
    private readonly connectToken: string;
    private readonly rtcConnection: RtcConnection;

    private socket: WebSocket | null;
    private requestIdIndex: number;
    private requests: Record<number, PendingRequest>;
    private notify: NotifyHandler;

    public state: Observable<ConnectionState>;
    private sessionInitialized: boolean;

    constructor(serverUrl: string, connectToken: string) {
        this.serverUrl = serverUrl;
        this.connectToken = connectToken;

        this.state = new Observable({ status: "unconnected" });
        this.requestIdIndex = 0;
        this.socket = null;
        this.requests = {};
        this.notify = {};

        this.rtcConnection = new RtcConnection();

        this.notify["NotifyIceCandidate"] = candidate => this.rtcConnection.applyRemoteIceCandidate(candidate);
        this.notify["NotifyIceCandidateFinished"] = () => this.rtcConnection.applyRemoteIceCandidate(null);
        this.notify["NotifyNegotiationOffer"] = async offer => {
            const answer = await this.rtcConnection.applyNegotiationOffer(offer);
            this.sendRequest("NegotiationAnswer", { answer });  
        };
    }

    public connect() {
        if(this.state.value.status !== "unconnected") {
            throw new Error("expected unconnected state");
        }

        const socket = this.socket = new WebSocket(this.serverUrl);
        this.socket.onopen = () => this.socket === socket && this.onSocketConnected();
        this.socket.onclose = e => this.socket === socket && this.onSocketClosed(e);
        this.socket.onerror = () => this.socket === socket && this.onSocketError();
        this.socket.onmessage = e => this.socket === socket && this.onSocketMessage(e);
        this.state.value = { status: "connecting" };
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

    private abortRequests() {
        for(const requestId of Object.keys(this.requests)) {
            const request = this.requests[parseInt(requestId)];
            delete this.requests[requestId];
            clearTimeout(request.timeout);
            request.resolve(kRequestErrorConnectionClosed);
        }
    }

    private async sendRequest<R extends C2SRequest["type"]>(request: R, payload: Omit<C2SRequest & { type: R }, "type">) : Promise<S2CResponse> {
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
    
    private initializeRtcSignalingConnection() {
        const connection = this;
        this.rtcConnection.applySignalingConnection(new class implements RtpSignalingConnection {
            async executeNegotiation() {
                const offer = await connection.rtcConnection.createLocalOffer();
                const response = await connection.sendRequest("NegotiationOffer", { offer });
                if(response.type === "NegotiationOfferSuccess") {
                    await connection.rtcConnection.applyNegotiationAnswer(response.payload.answer);
                    console.info("Executed negotiation.");
                } else {
                    console.error("Failed to execute negotiation: %o", response);
                }
            }

            async signalIceCandidates(candidates: IceCandidate[], finished: boolean): Promise<void> {
                const response = await connection.sendRequest("IceCandidates", { candidates, finished });
                if(response.type === "Success") {
                    return;
                }
                
                console.warn("Failed to signal local ICE candidates: %o", response);
            }

        });
    }

    private async onSocketConnected() {
        console.log("Connected. Creating offer and initializing session.");
        const offer = await this.rtcConnection.createLocalOffer();

        this.sendRequest("InitializeSesstion", {
            version: 1,
            token: this.connectToken,
            offer
        }).then(async result => {
            if(result.type !== "SessionInitializeSuccess") {
                // FIXME: Close/abort connection!
                console.warn("Invalid initialize session result: %o", result);
                return;
            }

            this.state.value = { status: "connected" };
            
            this.initializeRtcSignalingConnection();
            await this.rtcConnection.applyNegotiationAnswer(result.payload.answer);
            console.log("Applied remote description.");

            this.sessionInitialized = true;
            {
                const vcam = new VirtualCamera(30, { width: 1080, height: 1080, });
                vcam.start();

                const streamId = await this.rtcConnection.sendTrack(vcam.getMediaStream().getVideoTracks()[0]);
                console.log("Starting VCam at %s", streamId);
                this.sendRequest("BroadcastStart", { name: "V-Cam", source: streamId });
            }
        });
    }

    private onSocketClosed(event: CloseEvent) {
        this.state.value = { status: "disconnected", reason: event.reason ? `${event.code} / ${event.reason}` : `${event.code ?? 0}` };
        this.closeSocket();
        this.abortRequests();
    }

    private onSocketError() {
        this.state.value = { status: "failed", reason: "socket io failure" };
        this.closeSocket();
        this.abortRequests();
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
                const handler = this.notify[message.payload.type];
                if(typeof handler !== "function") {
                    console.warn("Received notify without a handler: %s", message.payload.type);
                    return;
                }

                // @ts-ignore
                handler(message.payload.payload, message.payload.type);
                break; 
        }
    }
}