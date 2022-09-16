import { BroadcastEntry, BroadcastKind, IceCandidate } from "../../../generated/rtp-messages";
import { EventEmitter } from "../../utils/ee2";
import { VirtualCamera } from "../../utils/virtual-camera";
import { RemoteStream, RtcConnection, RtpSignalingConnection } from "./rtc";
import { SignallingConnection, SignallingState } from "./signaling";

export interface RtpEvents {
    "rtp.new_remote_stream": string,

    "signalling.state_changed": [ newState: SignallingState, oldState: SignallingState ],
    "state_changed": [ newState: RtpConnectionState, oldState: RtpConnectionState ],


    "user.received": void,
    "user.joined": void,
    "user.left": void,


    // All broadcasts have been received.
    "broadcast.received": void,
    // A new broadcast has been created.
    "broadcast.created": [broadcast: Broadcast],
    "broadcast.ended": [broadcast: Broadcast],
}

type RtpConnectionState = "disconnected" | "connecting" | "initializing" | "connected";

export class Broadcast {
    // TODO: Type is it video or audio.
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

export class RtpConnection {
    readonly events: EventEmitter<RtpEvents>;
    
    private readonly connectToken: string;
    private readonly rtcConnection: RtcConnection;
    private readonly signallingConnection: SignallingConnection;
    private state: RtpConnectionState;

    private ownClientId: number;
    private ownUserId: number

    private broadcasts: Broadcast[];

    constructor(serverUrl: string, connectToken: string) {
        this.events = new EventEmitter();
        this.connectToken = connectToken;

        this.signallingConnection = new SignallingConnection(this.events, serverUrl);
        this.rtcConnection = new RtcConnection(this.events);

        this.state = "disconnected";
        this.broadcasts = [];

        this.events.on("signalling.state_changed", newState => {
            if(newState.status === "connected") {
                this.onSignallingConnected();
            }
        });

        this.signallingConnection.registerNotifyHandler("NotifyIceCandidate", candidate => this.rtcConnection.applyRemoteIceCandidate(candidate));
        this.signallingConnection.registerNotifyHandler("NotifyIceCandidateFinished", candidate => this.rtcConnection.applyRemoteIceCandidate(null));
        
        this.signallingConnection.registerNotifyHandler("NotifyNegotiationOffer", async offer => {
            const answer = await this.rtcConnection.applyNegotiationOffer(offer);
            this.signallingConnection.sendRequest("NegotiationAnswer", { answer });  
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

            this.broadcasts.push(broadcast);
            this.events.emit("broadcast.created", broadcast);
        });

        this.events.on("signalling.state_changed", newState => {
            switch(newState.status) {
                case "disconnected":
                case "failed":
                    if(this.state !== "disconnected") {
                        // TODO: Shutdown everything.
                        this.updateState("disconnected");
                    }
            }
        });

        // Currently debugging stuff
        {
            this.events.on("broadcast.created", broadcast => {
                this.subscribeBroadcast(broadcast.broadcastId);
            });
            this.events.on("broadcast.received", () => {
                for(const broadcast of this.broadcasts) {
                    this.subscribeBroadcast(broadcast.broadcastId);
                }
            });
        }
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
                // FIXME: Close/abort connection!
                console.warn("Invalid initialize session result: %o", result);
                this.updateState("disconnected");;
                return;
            }
            

            this.initializeRtcSignalingConnection();
            await this.rtcConnection.applyNegotiationAnswer(result.payload.answer);
            
            this.ownUserId = result.payload.own_user_id;
            this.ownClientId = result.payload.own_client_id;
            console.debug("Session initialized. Received client id %d (User Id %d)", this.ownClientId, this.ownUserId);
            this.updateState("connected");
            
            // TODO: Remove me: This is debug.
            {
                // {
                //     const vcam = new VirtualCamera(30, { width: 1080, height: 1080, });
                //     vcam.start();

                //     const streamId = await this.rtcConnection.sendTrack(vcam.getMediaStream().getVideoTracks()[0]);
                //     const result = await this.signallingConnection.sendRequest("BroadcastStart", { name: "V-Cam", source: streamId, kind: "Video" });
                //     console.log("V-Cam start result: %o (%s)", result, streamId);
                // }

                
                navigator.mediaDevices
                    .getUserMedia({ 
                        audio: {
                            deviceId: {
                                exact: "0c82b236ca3b62e137a4cd00ba7c8edd1aa4fc6ca9fb684bb1a556608f92f632x"
                            },
                            echoCancellation: {
                                exact: false
                            },
                        },
                        video: {
                            // "OBS-Camera" 05fff4c0016c994e756de288520758f63a7395d59be11d92d0a547e0dd37ae71
                            // "OBS Virtual Camera" 6418c3c2d86d10a351f0a9652b9fb6e5621136f62e188a04bde20d2bac6ab9b3
                            deviceId: {
                                exact: "6418c3c2d86d10a351f0a9652b9fb6e5621136f62e188a04bde20d2bac6ab9b3"
                            }
                        },
                    })
                    .then(async stream => {
                        const [ audioTrack ] = stream.getAudioTracks();
                        if(audioTrack) {
                            console.log("Got audio track. Sending it.");
                            const streamId = await this.rtcConnection.sendTrack(audioTrack);
                            const result = await this.signallingConnection.sendRequest("BroadcastStart", { name: "mic", source: streamId, kind: "Audio" });
                            console.log("Audio start result: %o (%s)", result, streamId);
                        }
                        
                        const [ videoTrack ] = stream.getVideoTracks();
                        if(videoTrack) {
                            console.log("Got video track. Sending it.");
                            const streamId = await this.rtcConnection.sendTrack(videoTrack);
                            const result = await this.signallingConnection.sendRequest("BroadcastStart", { name: "vid", source: streamId, kind: "Video" });
                            console.log("Video start result: %o (%s)", result, streamId);
                        }
                    })
                    .catch(error => {
                        console.error(error);
                    })
            }
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

    public connect() {
        if(this.state !== "disconnected") {
            throw new Error("already connected");
        }

        this.updateState("connecting");
        this.signallingConnection.connect();
    }
}