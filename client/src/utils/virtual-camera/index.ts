import { RenderProperties, VirtualCameraRenderer } from "./renderer";
import { RendererClock } from "./renderer-clock";

declare global {
    interface HTMLCanvasElement {
        captureStream(frames: number) : MediaStream;
    }
}

class FPSMeter {
    private frameCount;
    private lastFrameTimestamp;

    private avgFps;

    constructor() {
        this.frameCount = 0;
        this.lastFrameTimestamp = 0;
        this.avgFps = -1;
    }

    public getFPS() : number {
        return this.avgFps;
    }

    public getFrameCount() : number {
        return this.frameCount;
    }

    public frameBegin() {
        if(this.lastFrameTimestamp !== 0) {
            let fps = 1000 / (Date.now() - this.lastFrameTimestamp);
            if(this.avgFps === -1) {
                this.avgFps = fps;
            } else {
                this.avgFps = this.avgFps * .8 + fps * .2;
            }
        }
        this.lastFrameTimestamp = Date.now();
    }

    public frameEnd() {
        this.frameCount++;
    }
}

export class VirtualCamera {
    private readonly frameRate: number;
    private readonly canvas: HTMLCanvasElement;
    private readonly stream: MediaStream;

    private readonly frameMeter: FPSMeter;

    private baseTime: number;
    private canvasContext: CanvasRenderingContext2D;
    private running: boolean;
    private renderer: VirtualCameraRenderer;
    private renderInterval;

    public constructor(frameRate: number, bounds: { width: number, height: number }) {
        this.running = false;
        this.frameRate = frameRate;
        this.canvas = document.createElement("canvas");
        this.canvas.width = bounds.width;
        this.canvas.height = bounds.height;

        this.frameMeter = new FPSMeter();

        this.createCanvasContext();
        this.stream = this.canvas.captureStream(this.frameRate > 0 ? this.frameRate : 60);

        //this.renderer = new RendererSnow();
        //this.renderer = new RendererSnowSimplex();
        this.renderer = new RendererClock();
    }

    public start() {
        if(this.running) {
            return;
        }

        this.baseTime = Date.now();
        this.running = true;
        this.renderer.cameraStarted();
        this.renderInterval = setInterval(() => this.doRender(), this.frameRate >= 0 ? 1000 / this.frameRate : 50);
    }

    public stop() {
        if(!this.running) {
            return;
        }
        this.running = false;
        this.renderer.cameraStopped();
        clearInterval(this.renderInterval);
        this.renderInterval = undefined;
    }

    public getMediaStream() : MediaStream {
        return this.stream;
    }

    public getCanvas() {
        return this.canvas;
    }

    private createCanvasContext() {
        /* recreate, the old one may be crashed if we would use WebGL */
        this.canvasContext = this.canvas.getContext("2d");
        this.canvasContext.imageSmoothingEnabled = false;
    }

    private doRender() {
        if(!this.canvasContext) {
            this.createCanvasContext();
        }
        const ctx = this.canvasContext;

        this.frameMeter.frameBegin();
        let renderProperties: RenderProperties = {
            height: this.canvas.height,
            width: this.canvas.width,
            timestamp: Date.now()
        };

        ctx.clearRect(0, 0, renderProperties.width, renderProperties.height);
        this.renderer.render(ctx, renderProperties);
        false && this.renderDebugStats(ctx, renderProperties);

        this.frameMeter.frameEnd();
    }

    private renderDebugStats(ctx: CanvasRenderingContext2D, properties: RenderProperties) {
        let fontSize = 35;
        ctx.font = `${fontSize}px Consolas,monaco,monospace`;
        ctx.fillStyle = "magenta";
        ctx.strokeStyle = "black";
        ctx.lineWidth = 1;
        ctx.textAlign = "left";
        ctx.textBaseline = "top";

        let xOffset = 20;
        let yOffset = 20;
        {
            let text = "FPS: " + this.frameMeter.getFPS().toFixed(0);
            ctx.fillText(text, xOffset, yOffset);
            ctx.strokeText(text, xOffset, yOffset);

            xOffset += 5 * fontSize;
        }

        {
            if(xOffset + 6 * fontSize > properties.width) {
                xOffset = 20;
                yOffset += fontSize;
            }

            let text = "Time: ";
            ctx.fillText(text, xOffset, yOffset);
            ctx.strokeText(text, xOffset, yOffset);
            xOffset += 6 * fontSize;

            text = (Date.now() - this.baseTime).toString();
            ctx.fillText(text, xOffset, yOffset);
            ctx.strokeText(text, xOffset, yOffset);
            ctx.textAlign = "left";
            xOffset += fontSize;
        }

        {
            if(xOffset + 8 * fontSize > properties.width) {
                xOffset = 20;
                yOffset += fontSize;
            }

            let text = "Frame: ";
            ctx.fillText(text, xOffset, yOffset);
            ctx.strokeText(text, xOffset, yOffset);
            xOffset += 8 * fontSize;

            text = this.frameMeter.getFrameCount().toString();
            ctx.fillText(text, xOffset, yOffset);
            ctx.strokeText(text, xOffset, yOffset);
            ctx.textAlign = "left";
            xOffset += fontSize;
        }
    }
}




