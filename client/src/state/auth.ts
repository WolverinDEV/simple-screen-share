import { createAsyncThunk, createSlice } from "@reduxjs/toolkit";
import { AppState, appStore } from "./index";
import { persistReducer } from "redux-persist";
import { kReduxPersistLocalStorage } from "./storage";
import { axiosClient } from "../backend/axios";
import { PayloadAuthenticate, ResponseAuthenticate } from "../backend/rest-types";

export type AuthState = {
    /// We have been logged out.
    status: "invalided",
    reason: "unknown" | "invalidated"
} | {
    /// We have no session.
    status: "no-session",
} | {
    /// We have an active session.
    status: "valid",
    
    token: string,
    userId: string,
    userRegistered: boolean,
};

const slice = createSlice({
    name: "auth",
    initialState: (): AuthState => ({
        status: "no-session",
    }),
    reducers: {
        invalidateSession: (_state) => {
            return {
                status: "invalided",
                reason: "invalidated"
            };
        },
    },
    extraReducers: builder => {
        builder.addCase(initializeSession.fulfilled, (_state, action) => {
            const response = action.payload.response;
            switch(response.status) {
                case "Success":
                    return { status: "valid", token: response.refreshed_token, userId: response.user_id, userRegistered: response.user_registered };

                case "TokenExpired":
                    // Session expired.
                    return { status: "invalided", reason: "invalidated" };

                case "UserRequiresLogin":
                    // We need to log in.
                    return { status: "invalided", reason: "unknown" };

                case "NoAuthentication":
                    return { status: "no-session" };

                default:
                    return;
            }
        });
    }
});

export const initializeSession = createAsyncThunk<
    { response: ResponseAuthenticate },
    { mode: "create-or-existing" | "create" | "existing", credentials?: [ string, string ] },
    {
        state: AppState
    }
>("auth/initialize", async ({ mode, credentials }, { getState }) => {
    const authState = getState().auth;
    const headers = {};
    
    if(mode === "existing" || mode === "create-or-existing") {
        if(authState.status === "valid") {
            headers["Authorization"] = `Bearer ${authState.token}`;
        }
    }

    const { data } = await axiosClient.post<ResponseAuthenticate>(`/authenticate`, {
        create_account: mode === "create-or-existing" || mode === "create",
        credentials,
    } as PayloadAuthenticate, { headers });

    return { response: data };
});

export const isSessionValid = () => {
    return appStore.getState().auth.status === "valid";
}

export default persistReducer({
    key: "auth",
    storage: kReduxPersistLocalStorage
}, slice.reducer);

export const {
    invalidateSession
} = slice.actions;
