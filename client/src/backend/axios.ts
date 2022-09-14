import axios, { AxiosResponse } from "axios";
import { StaticConfig } from "../env";
import { appStore } from "../state";
import { invalidateSession } from "../state/auth";

const kMarkerAutoAuth = Symbol("axios-auto-auth");
const client = axios.create({
    baseURL: StaticConfig.restApiUrl
});

client.interceptors.request.use(config => {
    config.headers = config.headers || {};
    if("Authorization" in config.headers) {
        return config;
    }

    const { auth } = appStore.getState();
    if(auth.status !== "valid") {
        return config;
    }

    config.headers["Authorization"] = `Bearer ${auth.token}`;
    config[kMarkerAutoAuth] = true;
    return config;
});

client.interceptors.response.use(
    response => response,
        async error => {
    if(!axios.isAxiosError(error) || !error.response) {
        throw error;
    }

    handleAxiosResponse(error.response);
    throw error;
});

function handleAxiosResponse(response: AxiosResponse) {
    if(!response.config || !response.config[kMarkerAutoAuth]) {
        return;
    }

    if(response.status !== 401) {
        return;
    }

    appStore.dispatch(invalidateSession());
}

export const axiosClient = client;
