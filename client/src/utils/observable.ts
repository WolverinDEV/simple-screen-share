type Observer<T> = (newValue: T, oldValue: T) => void;
export class Observable<T> {
    private readonly observer: Observer<T>[];
    #value: T;

    constructor(initialValue: T) {
        this.observer = [];
        this.#value = initialValue;
    }

    get value() : T {
        return this.#value;
    }

    set value(target: T) {
        this.update(target);
    }

    public update(target: T) : T {
        const oldValue = this.value;
        this.#value = target;
        for(const observer of this.observer.slice()) {
            observer(target, oldValue);
        }
        return oldValue;
    }

    public subscribe(callback: Observer<T>) : () => void {
        const observer = this.observer;
        observer.push(callback);
        return () => {
            const index = observer.indexOf(callback);
            if(index !== -1) {
                observer.splice(index, 1);
            }
        };
    }

    public unsubscribe(callback: Observer<T>) : boolean {
        const index = this.observer.indexOf(callback);
        if(index === -1) {
            return false;
        }

        this.observer.splice(index, 1);
        return true;
    }
}
