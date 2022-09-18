import { BroadcastEntry, BroadcastKind, IceCandidate } from "../../../generated/rtp-messages";
import { EventEmitter } from "../../utils/ee2";
import { RemoteStream, RtcConnection, RtpSignalingConnection } from "./rtc";
import { SignallingConnection, SignallingState } from "./signaling";

export interface RtpEvents {
    "rtp.new_remote_stream": string,

    "signalling.state_changed": [ newState: SignallingState, oldState: SignallingState ],
    "state_changed": [ newState: RtpConnectionState, oldState: RtpConnectionState ],


    "client.received": void,
    "client.joined": Client,
    "client.left": Client,


    // All broadcasts have been received.
    "broadcast.received": void,
    // A new broadcast has been created.
    "broadcast.created": [broadcast: Broadcast],
    "broadcast.ended": [broadcast: Broadcast],

    "rtc.connection_state_changed": [ newState: RTCPeerConnectionState, oldState: RTCPeerConnectionState ], 
    "rtc.signaling_state_changed": [ newState: RTCSignalingState, oldState: RTCSignalingState ],
    "rtc.ice_connection_state_changed": [ newState: RTCIceConnectionState, oldState: RTCIceConnectionState ],
    "rtc.ice_gathering_state_changed": [ newState: RTCIceGathererState, oldState: RTCIceGathererState ],
}

export class Broadcast {
    public readonly broadcastId: number;
    public readonly name: string;
    public readonly clientId: number;
    public readonly kind: BroadcastKind;

    // The stream id is available when we subscribed to that broadcast.
    public streamId: string | null;

    constructor(info: BroadcastEntry) {
        this.broadcastId = info.broadcast_id;
        this.name = info.name;
        this.clientId = info.client_id;
        this.kind = info.kind;
    }
}

export class Client {
    public readonly clientId: number;
    public readonly userId: number;

    constructor(clientId: number, userId: number) {
        this.clientId = clientId;
        this.userId = userId;
    }
}

export type RtpConnectionState = "new" | "connecting" | "initializing" | "connected" | "failed" | "closed";
export class RtpConnection {
    readonly events: EventEmitter<RtpEvents>;
    
    private readonly connectToken: string;
    private readonly rtcConnection: RtcConnection;
    private readonly signallingConnection: SignallingConnection;
    private state: RtpConnectionState;

    private ownClientId: number;
    private ownUserId: number

    private broadcasts: Broadcast[];
    private users: Client[];

    constructor(serverUrl: string, connectToken: string) {
        this.events = new EventEmitter();
        this.connectToken = connectToken;

        this.signallingConnection = new SignallingConnection(this.events, serverUrl);
        this.rtcConnection = new RtcConnection(this.events);

        this.state = "new";
        this.broadcasts = [];
        this.users = [];

        this.signallingConnection.registerNotifyHandler("NotifyIceCandidate", candidate => this.rtcConnection.applyRemoteIceCandidate(candidate));
        this.signallingConnection.registerNotifyHandler("NotifyIceCandidateFinished", () => this.rtcConnection.applyRemoteIceCandidate(null));
        
        this.signallingConnection.registerNotifyHandler("NotifyNegotiationOffer", async offer => {
            const answer = await this.rtcConnection.applyNegotiationOffer(offer);
            this.signallingConnection.sendRequest("NegotiationAnswer", { answer });  
        });

        this.events.on("signalling.state_changed", newState => {
            if(this.state === "failed" || this.state === "closed") {
                return;
            }

            switch(newState.status) {
                case "connecting":
                    break;

                case "connected":
                    this.onSignallingConnected();
                    break;

                case "disconnected":
                    this.setConnectionFailed("signalling", `connection closed unexpectitly: ${newState.reason}`);
                    break;

                case "failed":
                    this.setConnectionFailed("signalling", `fatal error: ${newState.reason}`);
                    break;
            }
        });

        this.events.on("rtc.connection_state_changed", newState => {
            switch(newState) {
                case "closed":
                case "failed":
                    switch(this.state) {
                        case "failed":
                        case "closed":
                            break;

                        default:
                            this.setConnectionFailed("rtc", "rtc connection closed unexpectitly");
                            break;
                    }

                case "disconnected":
                    /* TODO: Timeout if no reconnect happens? */
                    break;

                case "connecting":
                case "connected":
                    /* nice */
                    break;

                case "new":
                default:
                    /* That's pretty odd... */
                    break;
            }
        });

        this.signallingConnection.registerNotifyHandler("NotifyBroadcasts", broadcasts => {
            for(const broadcastInfo of broadcasts) {
                const broadcast = new Broadcast(broadcastInfo);
                this.broadcasts.push(broadcast);
            }

            this.events.emit("broadcast.received");
        });
        this.signallingConnection.registerNotifyHandler("NotifyBroadcastStarted", async broadcastInfo => {
            const broadcast = new Broadcast(broadcastInfo);
            this.broadcasts.push(broadcast);
            this.events.emit("broadcast.created", broadcast);
        });

        this.signallingConnection.registerNotifyHandler("NotifyUsers", clients => {
            for(const clientInfo of clients) {
                const client = new Client(clientInfo.client_id, clientInfo.user_id);
                this.users.push(client);
            }

            this.events.emit("client.received");
        });

        this.signallingConnection.registerNotifyHandler("NotifyUserJoined", userInfo => {
            const client = new Client(userInfo.client_id, userInfo.user_id);
            this.users.push(client);
            this.events.emit("client.joined", client);
        });

        this.signallingConnection.registerNotifyHandler("NotifyUserLeft", clientId => {
            const clientIndex = this.users.findIndex(client => client.clientId === clientId);
            if(clientIndex === -1) {
                /* This is petty odd.. */
                return;
            }

            const [ client ] = this.users.splice(clientIndex, 1);
            this.events.emit("client.left", client);
        });
    }

    // Close the connection and free all resources.
    public close() {
        if(this.state === "closed") {
            return;
        }

        this.updateState("closed");
        this.doClose();
    }

    // Frees all resources but does not update the connection state.
    private doClose() {
        this.rtcConnection.close();
        this.signallingConnection.close();
    }

    public getState() : RtpConnectionState {
        return this.state;
    }

    private updateState(newState: RtpConnectionState) {
        if(this.state === newState) {
            return;
        }

        const oldState = this.state;
        this.state = newState;
        this.events.emit("state_changed", newState, oldState);
    }

    private setConnectionFailed(_module: string, _reason: string) {
        this.updateState("failed");
        this.doClose();
    }

    private async onSignallingConnected() {
        this.updateState("initializing");

        console.log("Signalling connected. Creating offer and initializing session.");
        const offer = await this.rtcConnection.createLocalOffer();

        this.signallingConnection.sendRequest("InitializeSesstion", {
            version: 1,
            token: this.connectToken,
            offer
        }).then(async result => {
            if(result.type !== "SessionInitializeSuccess") {
                console.warn("Invalid initialize session result: %o", result);
                this.setConnectionFailed("session-initialize", result.type);
                return;
            }
            

            this.initializeRtcSignalingConnection();
            await this.rtcConnection.applyNegotiationAnswer(result.payload.answer);
            
            this.ownUserId = result.payload.own_user_id;
            this.ownClientId = result.payload.own_client_id;
            console.debug("Session initialized. Received client id %d (User Id %d)", this.ownClientId, this.ownUserId);
            this.updateState("connected");
        });
    }

    private initializeRtcSignalingConnection() {
        const signallingConnection = this.signallingConnection;
        const rtcConnection = this.rtcConnection;
        this.rtcConnection.applySignalingConnection(new class implements RtpSignalingConnection {
            async executeNegotiation() {
                const offer = await rtcConnection.createLocalOffer();
                const response = await signallingConnection.sendRequest("NegotiationOffer", { offer });
                if(response.type === "NegotiationOfferSuccess") {
                    await rtcConnection.applyNegotiationAnswer(response.payload.answer);
                    console.info("Executed negotiation.");
                } else if(response.type === "NegotiationServerOfferPending") {
                    /* TODO: Reset local? */
                } else if(response.type === "ConnectionClosed") {
                    return;
                } else {
                    console.error("Failed to execute negotiation: %o", response);
                }
            }

            async signalIceCandidates(candidates: IceCandidate[], finished: boolean): Promise<void> {
                const response = await signallingConnection.sendRequest("IceCandidates", { candidates, finished });
                if(response.type === "Success" || response.type === "ConnectionClosed") {
                    return;
                }
                
                console.warn("Failed to signal %d local ICE candidates: %o", candidates.length, response);
            }
        });
    }
    
    public getRemoteStreams() : RemoteStream[] {
        return this.rtcConnection.getRemoteStreams();
    }

    public getRemoteStream(streamId: string) : RemoteStream | null {
        return this.rtcConnection.getRemoteStreams()
            .find(stream => stream.streamId === streamId) ?? null;
    }

    public getBroadcasts() : Broadcast[] {
        return this.broadcasts;
    }

    public getBroadcast(broadcastId: number) : Broadcast | null {
        return this.broadcasts.find(broadcast => broadcast.broadcastId === broadcastId) ?? null;
    }

    public async unsubscribeBroadcast(broadcastId: number) {
        const answer = await this.signallingConnection.sendRequest("BroadcastUnsubscribe", { broadcast_id: broadcastId });
        if(answer.type !== "Success") {
            console.warn("Failed to unsubscribe from broadcast %d: %s", broadcastId, answer.type);
        }

        const broadcast = this.getBroadcast(broadcastId);
        if(broadcast) {
            broadcast.streamId = undefined;
        }
    }

    public async subscribeBroadcast(broadcastId: number) : Promise<string> {
        const broadcast = this.getBroadcast(broadcastId);
        if(!broadcast) {
            throw new Error("invalid broadcast id");
        }

        if(broadcast.streamId) {
            return broadcast.streamId;
        }

        const answer = await this.signallingConnection.sendRequest("BroadcastSubscribe", { broadcast_id: broadcastId });
        if(answer.type === "BroadcastAlreadySubscribed") {
            /* subscribeBroadcast has been called multiple times. */
            if(broadcast.streamId) {
                return broadcast.streamId;
            }

            throw new Error("broadcast subscribed but we don't have a stream id");
        } else if(answer.type === "BroadcastSubscribed") {
            broadcast.streamId = answer.payload.stream_id;
            return broadcast.streamId;
        } else {
            throw new Error(`broadcast subscription failed ${answer.type}`);
        }
    }

    public getClients() : Client[] {
        return this.users;
    }

    public getClient(clientId: number) : Client | null {
        return this.users.find(client => client.clientId === clientId);
    }

    public connect() {
        if(this.state !== "new") {
            throw new Error("connect already called");
        }

        this.updateState("connecting");
        this.signallingConnection.connect();
    }

    public getRtcConnection() {
        return this.rtcConnection;
    }

    public getSignalingConnection() {
        return this.signallingConnection;
    }
}