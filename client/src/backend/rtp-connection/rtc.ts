import adapter from 'webrtc-adapter';
import { IceCandidate } from "../../../generated/rtp-messages";
import { VirtualCamera } from "../../utils/virtual-camera";

console.log("WebRTC adapter browserDetails %o", adapter.browserDetails);
export interface RtpSignalingConnection {
    executeNegotiation();

    signalIceCandidates(candidates: IceCandidate[], finished: boolean) : Promise<void>;
}

export class RemoteTrack {
    private readonly id: string;
    private readonly track: MediaStreamTrack;

    constructor(id: string, track: MediaStreamTrack) {
        this.id = id;
        this.track = track;
    }

    public getMediaTrack() : MediaStreamTrack {
        return this.track;
    }
}

export class LocalTrack {
    private readonly id: string;

}

export class RtcConnection {
    readonly peer: RTCPeerConnection;

    private signalingConnection: RtpSignalingConnection | null;

    private localIceCandidatesFinished: boolean;
    private remoteIceCandidatesFinished: boolean;
    private cachedLocalIceCandidates: IceCandidate[];
    private cachedRemoteIceCandidates: IceCandidate[];

    private freeLocalVideoStreams: [string, RTCRtpSender][];

    constructor() {
        this.signalingConnection = null;

        this.localIceCandidatesFinished = false;
        this.cachedLocalIceCandidates = [];

        this.remoteIceCandidatesFinished = false;
        this.cachedRemoteIceCandidates = [];


        this.peer = new RTCPeerConnection({
            iceServers: [{
                urls: 'stun:stun.l.google.com:19302'
            }],
        });

        // @ts-ignore
        window.peer = this.peer;

        
        this.peer.oniceconnectionstatechange = () => console.log("ICE connection state changed to %s", this.peer.iceConnectionState);
        this.peer.onsignalingstatechange = () => console.log("Signalling state changed to %s", this.peer.signalingState);
        this.peer.onconnectionstatechange = () => console.log("Connection state changed to %s", this.peer.connectionState);

        this.peer.onicegatheringstatechange = () => {
            console.log("ICE gathering state changed to %s", this.peer.iceGatheringState);
            if(this.peer.iceGatheringState === "complete") {
                this.onLocalIceFinished();
            }
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
        // FIXME: Handle!
        this.peer.onicecandidateerror = () => console.log("ICE candidates error.");
        this.peer.onnegotiationneeded = () => this.signalingConnection?.executeNegotiation();
        this.peer.ontrack = event => {
            // Track names are contained in the stream ids.
            if(event.streams.length !== 1) {
                console.warn(`Received not unique identifyable track (contained in ${event.streams.length} streams).`);
                return;
            }
            const trackId = event.streams[0].id;
            // readonly receiver: RTCRtpReceiver;
            // readonly streams: ReadonlyArray<MediaStream>;
            // readonly track: MediaStreamTrack;
            // readonly transceiver: RTCRtpTransceiver;
            console.log("Received track %s %s", event.track.kind, trackId);
            if(event.track.kind === "video") {
                const container = document.createElement("video");
                container.srcObject = event.streams[0];
                Object.assign(container.style, {
                    width: '500px',
                    height: '500px',
                    position: 'absolute',
                    top: '10px',
                    left: '10px',
                    zIndex: '500',
                    background: 'blue'
                });
                container.muted = true;
                container.autoplay = true;
                document.body.appendChild(container);
                container.play();
            }
        };

        this.freeLocalVideoStreams = [];
        for(let index = 0; index < 2; index++) {
            const stream = new MediaStream();
            const transceiver = this.peer.addTransceiver("video", {
                direction: "sendrecv",
                streams: [ stream ]
            });
            this.freeLocalVideoStreams.push([ stream.id, transceiver.sender ]);
        }
    }

    public async sendTrack(track: MediaStreamTrack) : Promise<string> {
        if(this.peer.connectionState !== "connected") {
            //throw new Error("peer not connected");
        }

        const [ streamId, sender ] = this.freeLocalVideoStreams.pop() ?? [];
        if(!sender) {
            // TODO: Renegotiate and allocate more slots.
            throw new Error("no free video stream slots");
        }

        sender.replaceTrack(track);
        // TODO: The server currently only knows this stream id if every data has been send on it.
        await new Promise(resolve => setTimeout(resolve, 3_000));
        return streamId;
    }

    public async applySignalingConnection(connection: RtpSignalingConnection) {
        this.signalingConnection = connection;
        this.processPendingIceCandidates();

        // {
        //     const vcam = new VirtualCamera(30, { width: 1920, height: 1080, });
        //     vcam.start();

        //     const _stream = new MediaStream();
            
        //     const stream = vcam.getMediaStream();
        //     const videoTrack = stream.getVideoTracks()[0];
        //     //const transceiver = this.peer.addTransceiver("video");
           
        //     const trackSender = this.peer.addTransceiver(videoTrack, {
        //         direction: "sendonly",
        //         streams: [ _stream ],
        //         // sendEncodings: [
        //         //     { rid: "h", maxBitrate: 1200 * 1024, scaleResolutionDownBy: 1, active: true },
        //         //     { rid: "m", maxBitrate:  600 * 1024, scaleResolutionDownBy: 2, active: true },
        //         //     { rid: "l", maxBitrate:  300 * 1024, scaleResolutionDownBy: 4, active: true }
        //         // ]
        //     });
        //     console.error("Stream ID: %s", _stream.id);
        //     console.error("Sender mid: %s", trackSender.mid);
        //     console.error("Parameters: %o", trackSender.sender.getParameters());
        //     //videoTrack.stop();
        //     // setTimeout(() => { 
        //     //     const vcam = new VirtualCamera(30, { width: 1920, height: 1080, });
        //     //     vcam.start();
        //     //     const vtrack = vcam.getMediaStream().getVideoTracks()[0];

        //     //     //this.peer.removeTrack(st);
        //     //     st = this.peer.addTrack(vtrack, _stream);
        //     //  }, 5_000);
        //     // console.error("Virtual Stream: %o", _stream.id);
        // }
    }

    /**
     * Create and apply a local offer.
     * This method can be called without a signaling connection.
     * Gathered local ICE-Candidates will be cached.
     * @returns local sdp offer
     */
    public async createLocalOffer() : Promise<string> {
        const offer = await this.peer.createOffer({ offerToReceiveAudio: true, offerToReceiveVideo: true });
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
            });
        } else {
            await this.peer.addIceCandidate(undefined);
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
            // TODO: Debounce candidate messages
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
}