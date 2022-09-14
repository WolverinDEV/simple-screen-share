import { Box, CircularProgress, Typography } from "@mui/material";
import React, { useEffect } from "react";
import { useMutation } from "react-query";
import { appStore, useAppSelector } from "../../../state";
import { initializeSession } from "../../../state/auth";

export const AuthentificationGuard = React.memo((props: {
    children: React.ReactNode | React.ReactNode[]
}) => {
    const authState = useAppSelector(state => state.auth);

    const { mutateAsync, isLoading, isError, isSuccess, error } = useMutation({
        mutationKey: "auth/initialize",
        mutationFn: async () => await appStore.dispatch(initializeSession({ mode: "create-or-existing" })).unwrap()
    });

    useEffect(() => {
        mutateAsync();
    }, [ ]);

    let content;
    if(isLoading) {
        content = <CircularProgress key="loading" />;
    } else if(isError) {
        content = <Typography key="error">Failed to initialize session.<br/>Please reload.</Typography>;
    } else if(isSuccess) { 
        if(authState.status === "valid") {
            content = <React.Fragment key="valid">{props.children}</React.Fragment>;
        } else if(authState.status === "invalided") {
            // TODO: Add login section (and register/new account)
            content = <Typography key="logged-out">Session logged out</Typography>
        } else if(authState.status === "no-session") {
            // TODO: Add create or login selection
            content = <Typography key="no-session">No session. Create one?</Typography>
        };
    }

    return (
        <Box sx={{
            alignSelf: "center",
            margin: "auto",
        }}>
            {content}
        </Box>
    );
});