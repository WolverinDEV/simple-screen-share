import { RenderProperties, VirtualCameraRenderer } from "./renderer";

/* Well this renderer has been inspired by the song Time Is Ticking (Secret Layor Remix) (https://www.youtube.com/watch?v=QmWzC9XYiIQ) */
 export class RendererClock extends VirtualCameraRenderer {
    private readonly baseTime = Date.now();
    private smoothSecondPointer = true;
    private fuckingFastClockBecauseIveNoTimeToWait = false;

    render(canvas: CanvasRenderingContext2D, properties: RenderProperties) {
        canvas.fillStyle = "white";
        canvas.fillRect(0, 0, properties.width, properties.height);

        /* The outer circle */
        {
            canvas.beginPath();
            for(let sub of [10, 11, 12]) {
                canvas.ellipse(properties.width / 2, properties.height / 2, properties.width / 2 - sub, properties.height / 2 - sub, 0, 0, 360);
            }
            canvas.stroke();
            canvas.closePath();
        }

        /* The numbers */
        {
            canvas.textAlign = "center";
            canvas.textBaseline = "middle";
            canvas.font = `20px Consolas,monaco,monospace`;
            canvas.strokeStyle = "black";
            canvas.fillStyle = "black";

            let numberDistance = 45;
            for(let number = 1; number <= 12; number++) {
                const index = number / 6 * Math.PI;
                const x = Math.sin(index) * (properties.width / 2 - numberDistance) + properties.width / 2;
                const y = -Math.cos(index) * (properties.height / 2 - numberDistance) + properties.height / 2;

                canvas.fillText(number.toString(), x, y);
            }

            let lineDistance = 20;
            canvas.lineWidth = 2;
            canvas.beginPath();
            for(let minute = 0; minute < 60; minute++) {
                const index = minute / 30 * Math.PI;
                let distance = lineDistance;
                if(minute % 5 === 0) {
                    distance *= 1.5;
                }

                const sx = Math.sin(index) * (properties.width / 2 - 12) + properties.width / 2;
                const sy = -Math.cos(index) * (properties.height / 2 - 12) + properties.height / 2;
                const dx = Math.sin(index) * (properties.width / 2 - distance) + properties.width / 2;
                const dy = -Math.cos(index) * (properties.height / 2 - distance) + properties.height / 2;

                canvas.moveTo(sx, sy);
                canvas.lineTo(dx, dy);
            }
            canvas.stroke();
            canvas.closePath();
        }

        /* The pointers */
        {
            let currentTimestamp: Date;
            if(this.fuckingFastClockBecauseIveNoTimeToWait) {
                currentTimestamp = new Date(Date.now() + (Date.now() - this.baseTime) * 100);
            } else {
                currentTimestamp = new Date();
            }
            canvas.beginPath();

            /* the hour pointer */
            {
                let distance = 120;
                canvas.lineWidth = 8;
                canvas.lineCap = "round";

                let index;
                if(this.smoothSecondPointer) {
                    index = ((currentTimestamp.getHours() % 12) * 3600 + currentTimestamp.getMinutes() * 60 + currentTimestamp.getSeconds()) / (6 * 60 * 60) * Math.PI;
                } else {
                    index = currentTimestamp.getHours() % 12 / 6 * Math.PI;
                }
                const x = Math.sin(index) * (properties.width / 2 - distance) + properties.width / 2;
                const y = -Math.cos(index) * (properties.height / 2 - distance) + properties.height / 2;
                canvas.moveTo(properties.width / 2, properties.height / 2);
                canvas.lineTo(x, y);
                canvas.stroke();
            }
            canvas.closePath();

            canvas.beginPath();
            {
                let distance = 70;
                canvas.lineWidth = 6;
                canvas.lineCap = "round";

                let index;
                if(this.smoothSecondPointer) {
                    index = (currentTimestamp.getMinutes() * 60 + currentTimestamp.getSeconds()) / (30 * 60) * Math.PI;
                } else {
                    index = currentTimestamp.getMinutes() / 30 * Math.PI;
                }
                const x = Math.sin(index) * (properties.width / 2 - distance) + properties.width / 2;
                const y = -Math.cos(index) * (properties.height / 2 - distance) + properties.height / 2;
                canvas.moveTo(properties.width / 2, properties.height / 2);
                canvas.lineTo(x, y);
                canvas.stroke();
            }
            canvas.closePath();

            canvas.beginPath();
            {
                let distance = 25;
                canvas.lineWidth = 3;
                canvas.lineCap = "round";

                let index;
                if(this.smoothSecondPointer) {
                    index = (currentTimestamp.getSeconds() * 1000 + currentTimestamp.getMilliseconds()) / 30000 * Math.PI;
                } else {
                    index = currentTimestamp.getSeconds() / 30 * Math.PI;
                }
                const x = Math.sin(index) * (properties.width / 2 - distance) + properties.width / 2;
                const y = -Math.cos(index) * (properties.height / 2 - distance) + properties.height / 2;
                canvas.moveTo(properties.width / 2, properties.height / 2);
                canvas.lineTo(x, y);
                canvas.stroke();
            }
            canvas.closePath();

            canvas.beginPath();
            canvas.ellipse(properties.width / 2, properties.height / 2, 10, 10, 0, 0, 360);
            canvas.fill();
            canvas.closePath();
        }

        /* the brand */
        if(Math.floor(properties.timestamp / (10 * 1000)) % 2 === 0) {
            canvas.fillText("A non sober renderer", properties.width / 2, properties.height * .60);
            canvas.fillText("created by WolverinDEV", properties.width / 2, properties.height * .60 + 20);
            canvas.fillText("14/10/20 1:50 AM", properties.width / 2, properties.height * .60 + 20 * 2);
        } else {
            canvas.fillText("Time is ticking...", properties.width / 2, properties.height * .60);
            canvas.fillText("and my heart is beating", properties.width / 2, properties.height * .60 + 20);
            canvas.fillText("come time is ticking", properties.width / 2, properties.height * .60 + 40);
        }
    }
}