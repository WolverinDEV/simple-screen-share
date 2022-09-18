import { Typography } from "@mui/material";
import { Box } from "@mui/system";
import React, { useContext, useEffect, useMemo, useRef } from "react";
import { useQuery } from "react-query";
import { Navigate, Route, Routes, useParams } from "react-router";
import { ResponseRoomJoin } from "../../../../generated/rest-types";
import { axiosClient } from "../../../backend/axios";
import { RtpConnection, RtpEvents } from "../../../backend/rtp-connection";
import { RemoteStream } from "../../../backend/rtp-connection/rtc";
import { extractErrorMessage } from "../../../utils/error";
import { useForceUpdate } from "../../hooks/force-render";

export default React.memo(() => {
    return (
        <Routes>
            <Route path={"/:roomId"} element={<PageRoom />} />
            <Route path="*" element={<Navigate to={"/"} />} />
        </Routes>
    )
});

const PageRoom = React.memo(() => {
    const { roomId } = useParams();
    if(!roomId) {
        throw new Error("missing roomId");
    }

    const { data, error, isLoading, isSuccess, isError } = useQuery({
        queryKey: [ "room/join", roomId ],
        queryFn: async () => {
            const { data } = await axiosClient.post<ResponseRoomJoin>(`room/${roomId}/join`);
            return data;
        },
    });

    if(isError) {
        return (
            <RoomJoinError
                key={"error"}
                roomId={roomId}
                error={extractErrorMessage(error) ?? "Unknown request error."}
            />
        );
    } else if(isLoading) {
        return (
            <RoomJoinPending
                key={"joining"}
                roomId={roomId}
            />
        )
    } else if(!data || !isSuccess) {
        return (
            <RoomJoinError
                key={"error-no-data"}
                roomId={roomId}
                error={"Failed to load join data."}
            />
        );
    } else if(data.status !== "Success") {
        return (
            <RoomJoinError
                key={"error-join-failed"}
                roomId={roomId}
                error={data.status}
            />
        );
    } else {
        return (
            <JoinedRoom 
                token={data.client_token}
                serverUrl={data.server_url}
                key={"joined-" + data.client_token}
            />
        );
    }
});

const RoomJoinError = React.memo((props: {
    roomId: string,
    error: string
}) => {
    return (
        <Box>
            <Typography>Failed to join room.</Typography>
            <Typography>{props.error}</Typography>
        </Box>
    );
});

const RoomJoinPending = React.memo((props: {
    roomId: string,
}) => {
    return (
        <Typography>Joining room {props.roomId}...</Typography>
    );
});

const ContextRtpConnection = React.createContext<RtpConnection>(undefined);
const JoinedRoom = React.memo((props: { token: string, serverUrl: string }) => {
    const connection = useMemo(() => {
        const connection = new RtpConnection(props.serverUrl, props.token);
        connection.connect();
        // @ts-ignore
        window.connection = connection;
        
        connection.events.on("state_changed", async newState => {
            if(newState === "connected") {
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
                            const streamId = await connection.getRtcConnection().sendTrack(audioTrack);
                            const result = await connection.getSignalingConnection().sendRequest("BroadcastStart", { name: "mic", source: streamId, kind: "Audio" });
                            console.log("Audio start result: %o (%s)", result, streamId);
                        }
                        
                        const [ videoTrack ] = stream.getVideoTracks();
                        if(videoTrack) {
                            console.log("Got video track. Sending it.");
                            const streamId = await connection.getRtcConnection().sendTrack(videoTrack);
                            const result = await connection.getSignalingConnection().sendRequest("BroadcastStart", { name: "vid", source: streamId, kind: "Video" });
                            console.log("Video start result: %o (%s)", result, streamId);
                        }
                    })
                    .catch(error => {
                        console.error(error);
                    })
            }
        })

        // Currently debugging stuff
        {
            connection.events.on("broadcast.created", broadcast => {
                connection.subscribeBroadcast(broadcast.broadcastId);
            });
            connection.events.on("broadcast.received", () => {
                for(const broadcast of connection.getBroadcasts()) {
                    connection.subscribeBroadcast(broadcast.broadcastId);
                }
            });
        }

        return connection;
    }, [ props.token, props.serverUrl ]);

    const stateHistory = useMemo(() => new RtcStateHistory(connection), [ connection ]);
    console.log(stateHistory);
    
    return (
        <ContextRtpConnection.Provider value={connection}>
            <ConnectionConnecting />
            <ConnectionConnected />
            <ConnectionDisconnected />
        </ContextRtpConnection.Provider>
    );
});

const useConnectionState = () => {
    const connection = useContext(ContextRtpConnection);
    const forceUpdate = useForceUpdate();
    useEffect(() => connection.events.on("state_changed", forceUpdate), [ connection ]);
    return connection.getState();
}


const ConnectionConnecting = React.memo(() => {
    const state = useConnectionState();
    if(state !== "connecting" && state !== "initializing") {
        return null;
    }

    return (
        <Typography>Connecting...</Typography>
    );
});

const ConnectionDisconnected = React.memo(() => {
    const state = useConnectionState();
    if(state !== "closed" && state !== "failed") {
        return null;
    }

    return (
        <Typography>Connection closed :(</Typography>
    );
});

const ConnectionConnected = React.memo(() => {
    const connection = useContext(ContextRtpConnection);
    const state = useConnectionState();
    if(state !== "connected") {
        return null;
    }

    return (
        <RemoteStreamsRenderer />
    );
});

const RemoteStreamsRenderer = React.memo(() => {
    const connection = useContext(ContextRtpConnection);
    const rerender = useForceUpdate();

    useEffect(() => connection.events.on("rtp.new_remote_stream", rerender), [ connection ]);

    const streams = connection.getRemoteStreams();
    return (
        <Box>
            <Typography>Remote stream count: {streams.length}</Typography>
            {streams.map(stream => (
                <RemoteStreamRenderer
                    key={stream.streamId}
                    streamId={stream.streamId}
                />
            ))}
        </Box>
    );
});

const RemoteStreamAudio = (props: {
    stream: RemoteStream
}) => {
    const { stream } = props;
    const refVideo = useRef<HTMLVideoElement>();
    useEffect(() => {
        if(!refVideo.current) {
            return;
        }

        const mstream = new MediaStream();
        mstream.addTrack(stream.track);
        refVideo.current.srcObject = mstream;
        refVideo.current.play().catch(() => console.warn("Failed to start audio playback for %s", stream.streamId));
    }, [ stream ]);

    return (
        <Box>
            <Box
                ref={refVideo}
                component={"video"}
            
                muted
                autoPlay

                controls={true}
            />
        </Box>
    )
};

const RemoteStreamVideo = (props: {
    stream: RemoteStream
}) => {
    const { stream } = props;

    const refVideo = useRef<HTMLVideoElement>();
    useEffect(() => {
        if(!refVideo.current) {
            return;
        }

        const mstream = new MediaStream();
        mstream.addTrack(stream.track);
        refVideo.current.srcObject = mstream;
        refVideo.current.play().catch(() => console.warn("Failed to start stream playback for %s", stream.streamId));
    }, [ stream ]);

    return (
        <Box sx={{
            position: "relative",
            
            width: '500px',
            height: '500px',
            background: 'blue',
        }}>
            <Box 
                ref={refVideo}
                component={"video"}

                sx={{
                    position: "absolute",
                    inset: 0,

                    height: "100%",
                    width: "100%",
                }}
                muted
                autoPlay
            />
            <Typography sx={{
                bgcolor: "grey.500",
                p: 1,

                position: "absolute",
                top: "0",
                left: "0",

                borderBottomRightRadius: "5px",
            }}>
                {stream.streamId}
            </Typography>
        </Box>
    );
}

const RemoteStreamRenderer = React.memo((props: {
    streamId: string,
}) => {
    const connection = useContext(ContextRtpConnection);

    const { streamId } = props;
    const stream = connection.getRemoteStream(streamId);
    if(!stream) {
        return <Typography>Missing stream.</Typography>
    };

    switch(stream.type) {
        case "video":
            return (
                <RemoteStreamVideo stream={stream} />
            );

        case "audio":
            return (
                <RemoteStreamAudio stream={stream} />
            );

        default:
            return <Typography>Unknown stream type {stream.type}</Typography>;
    }
});

interface StateTypeMapping {
    "ice-connection": RTCIceConnectionState,
    "ice-gathering": RTCIceGatheringState,
    "peer-connection": RTCPeerConnectionState,
    "peer-signaling": RTCSignalingState,
};

type StateType = keyof StateTypeMapping;
type StateChangeEntry<T extends StateType> = {
    type: T,
    timestamp: number,

    oldState: StateTypeMapping[T],
    newState: StateTypeMapping[T],
};

class RtcStateHistory {
    readonly connection: RtpConnection;
    readonly changes: StateChangeEntry<StateType>[]; 

    constructor(connection: RtpConnection) {
        this.connection = connection;

        this.changes = [];
        this.bindChangeHandler("rtc.connection_state_changed", "peer-connection");
        this.bindChangeHandler("rtc.signaling_state_changed", "peer-signaling");
        this.bindChangeHandler("rtc.ice_connection_state_changed", "ice-connection");
        this.bindChangeHandler("rtc.ice_gathering_state_changed", "ice-gathering");
    }

    private bindChangeHandler<T extends keyof RtpEvents, R extends StateType>(event: T, type: R) {
        this.connection.events.on(event as any, (newState: any, oldState: any) => {
            this.changes.push({
                type,
                timestamp: Date.now(),

                newState,
                oldState
            });
        });
    }
}