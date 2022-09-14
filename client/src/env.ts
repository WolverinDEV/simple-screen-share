function ensureVariable(value: string | undefined, name: string) : string {
    if(typeof value !== "string") {
        throw new Error(`Missing build config variable ${name}.`);
    }

    return value;
}

export const StaticConfig = {
    restApiUrl: ensureVariable(process.env.REST_API_URL, "REST_API_URL"),
};