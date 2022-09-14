import { RenderProperties, VirtualCameraRenderer } from ".";

function rgba(red: number, green: number, blue: number, alpha: number) {
    return ((alpha << 24) | (blue << 16) | (green << 8) | (red << 0)) >>> 0;
}

export const kSnowColors = [
    rgba(255, 255, 255, 255),
    rgba(220, 220, 220, 255),
    rgba(170, 170, 170, 255),
    rgba(120, 120, 120, 255),
    rgba(0, 0, 0, 255)
];

export class RendererSnow extends VirtualCameraRenderer {
    render(canvas: CanvasRenderingContext2D, properties: RenderProperties) {
        const data = canvas.getImageData(0, 0, properties.width, properties.height);
        if(data.data.byteOffset % 4 !== 0) {
            throw "image data byte offset is invalid";
        }

        const buffer = new Uint32Array(data.data.buffer, data.data.byteOffset / 4, data.data.byteLength / 4);
        for(let index = 0; index < buffer.length; index++) {
            buffer[index] = kSnowColors[(Math.random() * kSnowColors.length) | 0];
        }

        canvas.putImageData(data, 0, 0);
    }
}
