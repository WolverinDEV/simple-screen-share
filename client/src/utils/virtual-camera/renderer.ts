export type RenderProperties = {
    /** Time since epoch in milliseconds */
    timestamp: number,

    width: number,
    height: number
}

export abstract class VirtualCameraRenderer {
    public abstract render(canvas: CanvasRenderingContext2D, properties: RenderProperties);

    public cameraStarted() {}
    public cameraStopped() {}
}