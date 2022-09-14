import React, { CSSProperties } from "react";
import { CircularProgress } from "@mui/material";

const styleContainer: CSSProperties = {
    position: "fixed",
    inset: 0,

    display: "flex",
    flexDirection: "column",
    justifyContent: "center",

    background: "#121212",
};

const styleInner: CSSProperties = {
    alignSelf: "center",
};

export const LoadingScreen = React.memo(() => {
    /*
     * Just show a minimalistic loading spinner.
     * It might only flash for a few hundreds of a second.
     */
    return (
        <div style={styleContainer}>
            <div style={styleInner}>
                <CircularProgress />
            </div>
        </div>
    );
});
