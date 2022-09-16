
import * as sdpTransform from 'sdp-transform';
import _ from "lodash";

// This is currently only playing around,
// but the aim to to reduce the whole sdp description size.

type IceInfo = {
    fingerprint?: {
        type: string;
        hash: string;
    },
    options?: string,
    password?: string,
    username?: string,
};

function iceInfoFromMedia(media: sdpTransform.MediaDescription & { iceOptions?: string }) : IceInfo {
    return {
        fingerprint: media.fingerprint,
        options: media.iceOptions,
        password: media.icePwd,
        username: media.iceUfrag,
    };
}

type RTCPFeedback = {
    type: string,

    parameter?: string,
}

type RTCRtpCodecParameters = {
    payload_type: number,
    codec: string,

    rate?: number,
    encoding?: number,

    fmtp: string[],
    rtcp_feedback: RTCPFeedback[]
}

function mediaToCodeParameters(media: sdpTransform.MediaDescription) : RTCRtpCodecParameters[] {
    const codecs: Record<number, RTCRtpCodecParameters> = {};

    for(const codec of media.rtp) {
        codecs[codec.payload] = {
            codec: codec.codec,
            payload_type: codec.payload,

            rate: codec.rate,
            encoding: codec.encoding,

            fmtp: [],
            rtcp_feedback: []
        };
    }

    for(const fmtp of media.fmtp ?? []) {
        if(!(fmtp.payload in codecs)) {
            console.warn("fmtp line for codec %d is missing it's codec definition.", fmtp.payload);
            continue;
        }

        codecs[fmtp.payload].fmtp.push(fmtp.config);
    }

    for(const rtcp of media.rtcpFb ?? []) {
        if(!(rtcp.payload in codecs)) {
            console.warn("rtcp feedback line for codec %d is missing it's codec definition.", rtcp.payload);
            continue;
        }

        codecs[rtcp.payload].rtcp_feedback.push({
            type: rtcp.type,
            parameter: rtcp.subtype
        });
    }

    return Object.values(codecs);
}

type MediaLineInfo = {
    type: string,
    setup: string,
    direction: string,

    ssrcs: any,
    ssrcGroups: any,
};

function mediaToMediaLineInfo(media: sdpTransform.MediaDescription & { type: string }) : MediaLineInfo {
    return {
        type: media.type,
        setup: media.setup,
        direction: media.direction,

        ssrcs: media.ssrcs,
        ssrcGroups: media.ssrcGroups,
    }
}

export function compressSdp(sdp: string) {
    const parsed = sdpTransform.parse(sdp);

    const supportedCodes: Record<string, RTCRtpCodecParameters[]> = {};
    let iceInfo: IceInfo = undefined;
    const mediaLines: MediaLineInfo[] = [];
    const extMap: Record<string, any> = {};

    for(const media of parsed.media) {
        {
            const codecInfo = mediaToCodeParameters(media);
            if(media.type in supportedCodes) {
                if(!_.isEqual(supportedCodes[media.type], codecInfo)) {
                    console.warn("Supported codes missmatch for media %s. Expected: %o, Received: %o", media.type, media.rtp, supportedCodes[media.type]);
                }
            } else {
                supportedCodes[media.type] = codecInfo;
            }
        }

        {
            let mediaIceInfo = iceInfoFromMedia(media);
            if(!iceInfo) {
                iceInfo = mediaIceInfo;
            } else if(!_.isEqual(iceInfo, mediaIceInfo)) {
                console.warn("Media line %s has different ice info.", media.msid);
            }
        }

        mediaLines.push(mediaToMediaLineInfo(media));

        {
            if(media.type in extMap) {
                if(!_.isEqual(media.ext, extMap[media.type])) {
                    console.warn("Media line ext map does not matches for media %s.", media.type);
                    continue;
                }
            } else {
                extMap[media.type] = media.ext;
            }
        }
    }
    console.error(parsed);
    console.error("Codecs: %o, Ice: %o, Media: %o, Ext: %o", supportedCodes, iceInfo, mediaLines, extMap);

    console.log("%d -> %d", sdp.length, JSON.stringify([supportedCodes,iceInfo,mediaLines,extMap]).length);
}