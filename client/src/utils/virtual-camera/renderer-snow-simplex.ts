// @ts-ignore
import * as SimplexNoise from "simplex-noise";
import { RenderProperties, VirtualCameraRenderer } from ".";
import { kSnowColors } from "./renderer-snow";
export type RendererSnowSimplexConfiguration = {
    gridScale: number,
    timeScale: number
}

export class RendererSnowSimplex extends VirtualCameraRenderer {
    public static readonly ConfigSlow: RendererSnowSimplexConfiguration = { gridScale: 0.005, timeScale: 0.0001 };
    public static readonly ConfigSuperFast: RendererSnowSimplexConfiguration = { gridScale: .1, timeScale: 0.1 };
    public static readonly ConfigSuperFastRandom: RendererSnowSimplexConfiguration = { gridScale: 10, timeScale: 1 };

    private readonly config: RendererSnowSimplexConfiguration;
    private readonly noise: SimplexNoise;
    private readonly baseTimestamp: number;

    constructor() {
        super();
        this.config = RendererSnowSimplex.ConfigSlow;
        this.noise = new SimplexNoise();
        this.baseTimestamp = Date.now();
    }

    render(canvas: CanvasRenderingContext2D, properties: RenderProperties) {
        const data = canvas.getImageData(0, 0, properties.width, properties.height);
        if(data.data.byteOffset % 4 !== 0) {
            throw "image data byte offset is invalid";
        }

        const buffer = new Uint32Array(data.data.buffer, data.data.byteOffset / 4, data.data.byteLength / 4);
        for(let y = 0; y < data.height; y++) {
            for(let x = 0; x < data.width; x++) {
                buffer[y * data.width + x] = kSnowColors[Math.abs(this.noise.noise3D(x * this.config.gridScale, y * this.config.gridScale, (this.baseTimestamp - properties.timestamp) * this.config.timeScale) * kSnowColors.length) | 0];
            }
        }

        canvas.putImageData(data, 0, 0);
    }
}