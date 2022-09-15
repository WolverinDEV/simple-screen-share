export function extractErrorMessage(error: unknown): string | null {
    if(typeof error !== "object") {
        return error + "";
    }

    if(!error) {
        return null;
    }

    if(error instanceof Error) {
        return error.message;
    }

    return error.toString();
}