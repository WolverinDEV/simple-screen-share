import { Typography } from "@mui/material";
import React from "react";
import { Route, Routes } from "react-router";
import PageRoom from "./room";

export default React.memo(() => (
    <Routes>
        <Route path={"/room/*"} element={<PageRoom />} />
        <Route path="*" element={<Fallback />} />
    </Routes>
));

const Fallback = React.memo(() => {
    return (
        <Typography>Well, we're working on this page.</Typography>
    );
});