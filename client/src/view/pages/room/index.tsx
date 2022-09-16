import { Typography } from "@mui/material";
import { Box } from "@mui/system";
import React, { useContext, useEffect, useMemo, useRef } from "react";
import { useQuery } from "react-query";
import { Navigate, Route, Routes, useParams } from "react-router";
import { ResponseRoomJoin } from "../../../../generated/rest-types";
import { axiosClient } from "../../../backend/axios";
import { RtpConnection } from "../../../backend/rtp-connection";
import { SignallingConnection } from "../../../backend/rtp-connection/signaling";
import { extractErrorMessage } from "../../../utils/error";
import { useForceUpdate } from "../../hooks/force-render";
import { useObservable } from "../../hooks/observable";

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

        return connection;
    }, [ props.token, props.serverUrl ]);

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
    if(state !== "disconnected") {
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
                <RemoteStream
                    key={stream.streamId}
                    streamId={stream.streamId}
                />
            ))}
        </Box>
    );
});

const RemoteStream = React.memo((props: {
    streamId: string,
}) => {
    const connection = useContext(ContextRtpConnection);

    const { streamId } = props;
    const stream = connection.getRemoteStream(streamId);
    if(!stream) {
        return <Typography>Missing stream.</Typography>
    };

    if(stream.type === "audio") {
        return <Typography>Audio stream.</Typography>
    }

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
                {streamId}
            </Typography>
        </Box>
    )
});