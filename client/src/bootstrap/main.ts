import React from "react";
import ReactDOM, { Root } from "react-dom/client";
import { LoadingScreen } from "./loading";

function createReactDom() : Root {
    const container = document.createElement("div");
    document.body.appendChild(container);
    return ReactDOM.createRoot(container);;
}

async function main() {
    const reactRoot = createReactDom();
    reactRoot.render(React.createElement(LoadingScreen));

    const { main } = await import("../main");
    await main(reactRoot);
    
    console.log("Loaded main app.");
}

main().catch(error => {
    console.error(`Failed to startup app.`);
    console.error(error);
})