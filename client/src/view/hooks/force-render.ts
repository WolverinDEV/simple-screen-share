import { useCallback, useState } from "react";

export function useForceRender() : () => void {
    const [ _state, setState ] = useState<number>(0);
    return useCallback(() => {
        setState(performance.now());
    }, [ setState ]);
}