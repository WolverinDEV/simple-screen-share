import { configureStore } from '@reduxjs/toolkit'
import { TypedUseSelectorHook, useDispatch, useSelector } from "react-redux";
import { persistStore } from "redux-persist";
import { Persistor } from "redux-persist/es/types";
import {
    FLUSH as ReduxPersistFlush,
    REHYDRATE as ReduxPersistRehydrate,
    PAUSE as ReduxPersistPause,
    PERSIST as ReduxPersistPersist,
    PURGE as ReduxPersistPurge,
    REGISTER as ReduxPersistRegister
} from "redux-persist";
import reducerAuth from "./auth";

export const appStore = configureStore({
    reducer: {
        auth: reducerAuth,
    },
    middleware: getDefaultMiddleware => {
 
        return getDefaultMiddleware({
            serializableCheck: {
                ignoredActions: [
                    ReduxPersistFlush,
                    ReduxPersistRehydrate,
                    ReduxPersistPause,
                    ReduxPersistPersist,
                    ReduxPersistPurge,
                    ReduxPersistRegister
                ]
            }
        })
    }
});

let appPersistor: Persistor;
export async function initializeAppStore() {
    appPersistor = await new Promise<Persistor>(resolve => {
        let result = persistStore(appStore, null, () => resolve(result));
    });
}

export type AppState = ReturnType<typeof appStore.getState>
export type AppDispatch = typeof appStore.dispatch

export const useAppDispatch = () => useDispatch<AppDispatch>();
export const useAppSelector: TypedUseSelectorHook<AppState> = useSelector;
