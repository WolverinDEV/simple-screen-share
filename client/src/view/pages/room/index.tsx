import { Box, Typography } from "@mui/material";
import React, { useEffect, useMemo } from "react";
import { useMutation, useQuery } from "react-query";
import { Navigate, Route, Routes, useParams } from "react-router";
import { ResponseRoomJoin } from "../../../../generated/rest-types";
import { axiosClient } from "../../../backend/axios";
import { RtpServerConnection } from "../../../backend/rtp-connection/signaling";

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

    if(data && data.status === "Success") {
        return <JoinedRoom token={data.client_token} serverUrl={data.server_url} key="joined" />
    };
    console.log("Room join token: %o", data);

    return (
        <Typography>Joining room {roomId}...</Typography>
    );
});

const JoinedRoom = React.memo((props: { token: string, serverUrl: string }) => {
    const connection = useMemo(() => {
        const connection = new RtpServerConnection(props.serverUrl, props.token);
        connection.connect();
        // @ts-ignore
        window.connection = connection;

        return connection;
    }, [ props.token, props.serverUrl ]);

    if(connection.state.value.status === "connected") {

    }
    return (
        <Typography>Connecting...</Typography>
    );
});

const ContextRtpConnection = React.createContext<RtpServerConnection>(undefined);

const ConnectionConnecting = React.memo(() => {

});