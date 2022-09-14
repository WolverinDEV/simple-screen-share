import React from "react";
import { Root } from "react-dom/client";
import { RtpServerConnection } from "./backend/rtp-connection/signaling";
import { appStore, initializeAppStore } from "./state";
import { initializeSession } from "./state/auth";
import AppView from "./view";

export async function main(root: Root) {
    await initializeAppStore();

    root.render(React.createElement(AppView));
}