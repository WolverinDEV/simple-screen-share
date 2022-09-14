import { Observable } from "../../utils/observable";
import { useEffect, useState } from "react";

export const useObservable = <T>(observable: Observable<T>) => {
    const [ value, setValue ] = useState(() => observable.value);

    useEffect(() => {
        if(value !== observable.value) {
            /* value has changed, before we could render our element. */
            setValue(observable.value);
        }
        return observable.subscribe(value => setValue(value));
    }, [ observable ]);

    return value;
};
