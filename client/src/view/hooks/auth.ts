import { useAppSelector } from "../../state"
import { AuthState } from "../../state/auth";

export const useAuthInfo = (): AuthState & { status: "valid" } => {
    const state = useAppSelector(state => state.auth);
    if(state.status !== "valid") {
        throw new Error("Missing session");
    }

    return state;
}