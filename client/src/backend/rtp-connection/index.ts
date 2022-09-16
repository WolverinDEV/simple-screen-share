import { IceCandidate } from "../../../generated/rtp-messages";
import { EventEmitter } from "../../utils/ee2";
import { VirtualCamera } from "../../utils/virtual-camera";
import { RemoteStream, RtcConnection, RtpSignalingConnection } from "./rtc";
import { SignallingConnection, SignallingState } from "./signaling";

export interface RtpEvents {
    "rtp.new_remote_stream": string,

    "signalling.state_changed": [ newState: SignallingState, oldState: SignallingState ],
    "state_changed": [ newState: RtpConnectionState, oldState: RtpConnectionState ],
}

type RtpConnectionState = "disconnected" | "connecting" | "initializing" | "connected";

export class RtpConnection {
    readonly events: EventEmitter<RtpEvents>;
    
    private readonly connectToken: string;
    private readonly rtcConnection: RtcConnection;
    private readonly signallingConnection: SignallingConnection;
    private state: RtpConnectionState;

    constructor(serverUrl: string, connectToken: string) {
        this.events = new EventEmitter();
        this.connectToken = connectToken;

        this.signallingConnection = new SignallingConnection(this.events, serverUrl);
        this.rtcConnection = new RtcConnection(this.events);

        this.state = "disconnected";

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

        this.signallingConnection.registerNotifyHandler("NotifyUsers", users => console.info("Users: %o", users));
        this.signallingConnection.registerNotifyHandler("NotifyBroadcastStarted", broadcast => {
            console.info("Broadcast started: %o", broadcast);
            this.signallingConnection.sendRequest("BroadcastSubscribe", { broadcast_id: broadcast.broadcast_id });
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
                return;
            }
            
            this.initializeRtcSignalingConnection();
            await this.rtcConnection.applyNegotiationAnswer(result.payload.answer);
            console.log("Applied remote description.");

            this.updateState("connected");
            
            // TODO: Remove me: This is debug.
            {
                const vcam = new VirtualCamera(30, { width: 1080, height: 1080, });
                vcam.start();

                const streamId = await this.rtcConnection.sendTrack(vcam.getMediaStream().getVideoTracks()[0]);
                console.log("Starting VCam at %s", streamId);
                const result = await this.signallingConnection.sendRequest("BroadcastStart", { name: "V-Cam", source: streamId });
                console.log("V-Cam start result: %o", result);
                
                // await new Promise(resolve => setTimeout(resolve, 1000));
                // console.log("Starting VCam at %s", streamId);
                // await this.sendRequest("BroadcastStart", { name: "V-Cam 2", source: streamId });

                // await new Promise(resolve => setTimeout(resolve, 1000));
                // console.log("Starting VCam at %s", streamId);
                // await this.sendRequest("BroadcastStart", { name: "V-Cam", source: streamId });
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

    public connect() {
        if(this.state !== "disconnected") {
            throw new Error("already connected");
        }

        this.updateState("connecting");
        this.signallingConnection.connect();
    }
}