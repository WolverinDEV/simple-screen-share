import adapter from 'webrtc-adapter';
import { RtpEvents } from '.';
import { IceCandidate } from "../../../generated/rtp-messages";
import { EventEmitter } from '../../utils/ee2';
import { compressSdp } from './sdp';

// Known bugs which need to be detected:
// - Firefox ice candidate gathering may hangs up (browser restart required).
//   Detectable via no ice candidate events fires and connection failed.
console.log("WebRTC adapter browserDetails %o", adapter.browserDetails);

interface EventIceCandidateError extends Event {
    address: string,
    errorCode: number,
    errorText: string,
    port: number,
    url: string
}

export interface RtpSignalingConnection {
    executeNegotiation();

    signalIceCandidates(candidates: IceCandidate[], finished: boolean) : Promise<void>;
}

type StreamType = "audio" | "video";

export class RemoteStream {
    readonly type: StreamType;
    readonly streamId: string;
    readonly track: MediaStreamTrack;

    constructor(
        type: StreamType,
        streamId: string,
        track: MediaStreamTrack,
    ) {
        this.type = type;
        this.streamId = streamId;
        this.track = track;
    }
}

class LocalStream {
    readonly type: StreamType;
    readonly streamId: string;
    readonly sender: RTCRtpSender;

    // Media stream used for allocating the transceiver.
    readonly mediaStream: MediaStream;

    constructor(mediaStream: MediaStream, type: StreamType, sender: RTCRtpSender) {
        this.mediaStream = mediaStream;
        this.type = type;
        this.streamId = mediaStream.id;
        this.sender = sender;
    }

    public setTrack(track: MediaStreamTrack) {
        this.sender.replaceTrack(track);
    }

    public removeTrack() {
        if(this.sender.transport.state === "closed") {
            return;
        }
        
        this.sender.replaceTrack(null);
    }
}

export class RtcConnection {
    private readonly events: EventEmitter<RtpEvents>;
    readonly peer: RTCPeerConnection;

    private signalingConnection: RtpSignalingConnection | null;

    private localIceCandidatesFinished: boolean;
    private remoteIceCandidatesFinished: boolean;
    private cachedLocalIceCandidates: IceCandidate[];
    private cachedRemoteIceCandidates: IceCandidate[];

    private localStreams: LocalStream[];
    private remoteStreams: RemoteStream[];

    private lastConnectionState: RTCPeerConnectionState;
    private lastSignallingState: RTCSignalingState;
    private lastIceConnectionState: RTCIceConnectionState;
    private lastIceGatheringState: RTCIceGathererState;
   
    constructor(events: EventEmitter<RtpEvents>) {
        this.events = events;
        this.signalingConnection = null;

        this.localIceCandidatesFinished = false;
        this.cachedLocalIceCandidates = [];

        this.remoteIceCandidatesFinished = false;
        this.cachedRemoteIceCandidates = [];

        this.localStreams = [];
        this.remoteStreams = [];

        this.peer = new RTCPeerConnection({
            iceServers: [{
                urls: 'stun:stun.l.google.com:19302'
            }],
        });

        // @ts-ignore
        window.peer = this.peer;

        this.peer.onsignalingstatechange = () => {
            this.events.emit("rtc.signaling_state_changed", this.peer.signalingState, this.lastSignallingState);
            this.lastSignallingState = this.peer.signalingState;
        };
        this.peer.onconnectionstatechange = () => {
            this.events.emit("rtc.connection_state_changed", this.peer.connectionState, this.lastConnectionState);
            this.lastConnectionState = this.peer.connectionState;
        };
        this.peer.oniceconnectionstatechange = () => {
            this.events.emit("rtc.ice_connection_state_changed", this.peer.iceConnectionState, this.lastIceConnectionState);
            this.lastIceConnectionState = this.peer.iceConnectionState;
        };
        this.peer.onicegatheringstatechange = () => {
            this.events.emit("rtc.ice_gathering_state_changed", this.peer.iceGatheringState, this.lastIceGatheringState);
            this.lastIceGatheringState = this.peer.iceGatheringState;
        };
        this.peer.onicecandidate = event => {
            if(!event.candidate || !event.candidate.candidate) {
                this.onLocalIceFinished();
                return;
            }

            const candidate = event.candidate;
            if(("sdpMLineIndex" in candidate && candidate.sdpMLineIndex !== 0) || ("sdpMid" in candidate && candidate.sdpMid !== "0")) {
                return;
            }

            this.onLocalIceCandidate({
                candidate: candidate.candidate.startsWith("candidate:") ? candidate.candidate.substring(10) : candidate.candidate,
                ufrag: candidate.usernameFragment ?? ""
            });
        };
        
        this.peer.onicecandidateerror = (event: EventIceCandidateError) => {
            if(adapter.browserDetails.browser === "firefox") {
                /*
                 * Firefox does not provides any information about which ICE candidate fails.
                 * Bug: https://bugzilla.mozilla.org/show_bug.cgi?id=1561441
                 */
                return;
            }


            console.log("ICE candidate error: %o", {
                address: event.address,
                errorCode: event.errorCode,
                errorText: event.errorText,
                port: event.port,
                url: event.url
            });

            // TODO: How to propergate this error/some special handling?
        };
        this.peer.onnegotiationneeded = () => this.signalingConnection?.executeNegotiation();
        this.peer.ontrack = event => {
            // Track names are contained in the stream ids.
            if(event.streams.length !== 1) {
                /*
                 * This might happen when we offer to receive video/audio but the server does not send any video/audio streams.
                 * The client will still create tracks for that but without any stream ids.
                 */
                console.warn(`Received not unique identifyable ${event.track.kind} track (contained in ${event.streams.length} streams).`);
                return;
            }

            const trackId = event.streams[0].id;
            const remoteStream = new RemoteStream(
                event.track.kind === "video" ? "video" : "audio",
                trackId,
                event.track
            );

            this.remoteStreams.push(remoteStream);
            this.events.emit("rtp.new_remote_stream", remoteStream.streamId);

            // readonly receiver: RTCRtpReceiver;
            // readonly streams: ReadonlyArray<MediaStream>;
            // readonly track: MediaStreamTrack;
            // readonly transceiver: RTCRtpTransceiver;
            console.log("Received track %s %s", event.track.kind, trackId);
        };

        this.lastConnectionState = this.peer.connectionState;
        this.lastSignallingState = this.peer.signalingState;
        this.lastIceConnectionState = this.peer.iceConnectionState;
        this.lastIceGatheringState = this.peer.iceGatheringState;

        this.events.on("rtc.ice_gathering_state_changed", newState => {
            if(newState === "complete") {
                /*
                 * Not all implementations fire an ice candidate with "undefined" description
                 * to indicate that gathering has been completed. 
                 */
                this.onLocalIceFinished();
            }
        });

        this.preallocateLocalStreams();
    }

    private preallocateLocalStreams() {
        for(let index = 0; index < 2; index++) {
            this.allocateLocalStream("video");
            this.allocateLocalStream("audio");
        }
    }

    public close() {
        this.peer.close();
        for(const localStream of this.localStreams) {
            localStream.removeTrack();
        }
        this.localStreams.splice(0, this.localStreams.length);
        this.remoteStreams.splice(0, this.remoteStreams.length);
    }

    private allocateLocalStream(type: StreamType) : LocalStream {
        const mediaStream = new MediaStream();
        const transceiver = this.peer.addTransceiver(type, {
            direction: "sendrecv",
            streams: [ mediaStream ],
        });

        const localStream = new LocalStream(mediaStream, type, transceiver.sender);
        this.localStreams.push(localStream);
        return localStream;
    }

    public sendTrack(track: MediaStreamTrack) : string {
        let type: StreamType;
        if(track.kind === "video") {
            type = "video";
        } else if(track.kind === "audio") {
            type = "audio";
        } else {
            throw new Error(`unsupported track kind ${track.kind}`);
        }

        let localStream = this.localStreams.find(stream => stream.type === type && !stream.sender.track);
        if(!localStream) {
            localStream = this.allocateLocalStream(type);
        }

        localStream.setTrack(track);
        return localStream.streamId;
    }

    public stopSending(track: MediaStreamTrack | string) {
        if(typeof track === "string") {
            const localStream = this.localStreams.find(stream => stream.streamId === track);
            localStream?.removeTrack();
        } else {
            const localStreams = this.localStreams.filter(stream => stream.sender.track === track);
            for(const localStream of localStreams) {
                localStream.removeTrack();
            }
        }
    }

    public async applySignalingConnection(connection: RtpSignalingConnection) {
        this.signalingConnection = connection;
        this.processPendingIceCandidates();
    }

    /**
     * Create and apply a local offer.
     * This method can be called without a signaling connection.
     * Gathered local ICE-Candidates will be cached.
     * @returns local sdp offer
     */
    public async createLocalOffer() : Promise<string> {
        const offer = await this.peer.createOffer({ /* offerToReceiveVideo: true */ });
        await this.motifyLocalSdp(offer);
        
        console.groupCollapsed("SDP local offer");
        console.debug("%s", offer.sdp);
        console.groupEnd();

        await this.peer.setLocalDescription(offer);
        return offer.sdp!;
    }

    /**
     * Apply remote negotiation answer
     * @param answer remote sdp answer
     */
    public async applyNegotiationAnswer(answer: string) : Promise<void> {
        let init: RTCSessionDescriptionInit = { sdp: answer, type: "answer" };
        await this.motifyRemoteSdp(init);

        console.groupCollapsed("SDP remote answer");
        console.debug("%s", init.sdp);
        console.groupEnd();

        await this.peer.setRemoteDescription({ sdp: answer, type: "answer" });
    }

    /**
     * Apply a remote negotiation offer and generate a local response.
     * @param offer remote sdp offer
     * @returns local sdp answer
     */
    public async applyNegotiationOffer(offer: string) : Promise<string> {
        {
            let init: RTCSessionDescriptionInit = { sdp: offer, type: "offer" };
            await this.motifyRemoteSdp(init);

            console.groupCollapsed("SDP remote offer");
            console.debug("%s", init.sdp);
            console.groupEnd();

            await this.peer.setRemoteDescription(init);
        }

        const answer = await this.peer.createAnswer({});

        console.groupCollapsed("SDP local answer");
        console.debug("%s", answer.sdp);
        console.groupEnd();

        await this.peer.setLocalDescription(answer);
        return answer.sdp!;
    }

    /**
     * Add a remote ICE candidate.
     * If the current signaling connection is `null` the candidate will be cached and applied as soon
     * the signaling connection is provided.
     * @param candidate Target candidate to apply or `null` if candidate gathering has been finished.
     */
    public async applyRemoteIceCandidate(candidate: IceCandidate | null) : Promise<void> {
        if(!this.signalingConnection) {
            // Cache remote ICE candidate and apply as soon we've a signaling connection.
            if(candidate) {
                this.cachedRemoteIceCandidates.push(candidate);
            } else {
                this.remoteIceCandidatesFinished = true;
            }
            return;
        }

        this.addRemoteIceCandidate(candidate);
    }

    private async addRemoteIceCandidate(candidate: IceCandidate | null) {
        if(candidate) {
            await this.peer.addIceCandidate({
                sdpMLineIndex: 0,
                candidate: `candidate:${candidate.candidate}`,
                usernameFragment: candidate.ufrag
            }).catch(error => {
                console.warn(`Failed to add remote ice candidate ${candidate.candidate}: %s`, error.message);
            });
        } else {
            await this.peer.addIceCandidate(undefined).catch(error => {
                console.warn(`Failed to add remote ice candidate finish: %s`, error.message);
            });
        }
    }

    private async onLocalIceFinished() {
        if(this.localIceCandidatesFinished) {
            return;
        }
        this.localIceCandidatesFinished = true;

        if(this.signalingConnection) {
            await this.signalingConnection.signalIceCandidates([], true);
        }
    }
    
    private async onLocalIceCandidate(candidate: IceCandidate) {
        if(this.signalingConnection) {
            await this.signalingConnection.signalIceCandidates([ candidate ], false);
        } else {
            this.cachedLocalIceCandidates.push(candidate);
        }
    }

    private async processPendingIceCandidates() {
        let promises: Promise<void>[] = [];
        if(this.cachedLocalIceCandidates.length > 0 || this.remoteIceCandidatesFinished) {
            const candidates = this.cachedLocalIceCandidates.splice(0, this.cachedLocalIceCandidates.length);
            promises.push(this.signalingConnection.signalIceCandidates(candidates, this.remoteIceCandidatesFinished));
        }

        promises.push((async () => {
            for(const candidate of this.cachedRemoteIceCandidates) {
                await this.applyRemoteIceCandidate(candidate);
            }

            if(this.localIceCandidatesFinished) {
                await this.applyRemoteIceCandidate(null);
            }
        })());

        // Executing sending & local applying in parallel.
        await Promise.all(promises);
    }

    // Modify local sdp before applying
    private async motifyLocalSdp(sdp: RTCSessionDescriptionInit) {
        //setTimeout(() => console.log("Compressed: %o", compressSdp(sdp.sdp!)));
    }

    // Modify remote sdp before applying
    private async motifyRemoteSdp(sdp: RTCSessionDescriptionInit) {
        const lines = sdp.sdp.split("\n");
        for(let index = 0; index < lines.length; index++) {
            if(lines[index].startsWith("m=")) {
                // https://datatracker.ietf.org/doc/html/rfc8866#section-5.8
                lines.splice(index + 1, 0, "b=AS:512");
                index++;
            }
        }
        // x-google-max-bitrate=2500
        sdp.sdp = lines.join("\n");
    }

    public getRemoteStreams() : RemoteStream[] {
        return this.remoteStreams;
    }

    public getSignalingState() : RTCSignalingState {
        return this.peer.signalingState;
    }

    public getIceConnectionState() : RTCIceConnectionState {
        return this.peer.iceConnectionState;
    }

    public getIceGatheringState() : RTCIceGatheringState {
        return this.peer.iceGatheringState;
    }

    public getConnectionState() {
        return this.peer.connectionState;
    }
}