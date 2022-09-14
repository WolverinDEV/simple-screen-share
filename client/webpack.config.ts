import path from "node:path";
import { Configuration } from "webpack";
import HtmlWebpackPlugin from "html-webpack-plugin";
import ReactRefreshPlugin from "@pmmmwh/react-refresh-webpack-plugin";
import DotEnvPlugin from "dotenv-webpack";

const isDevelopment = process.env.NODE_ENV === "development";
console.log(`Creating WebPack configuration for ${isDevelopment ? "development" : "production"}.`)
export default {
    entry: path.join(__dirname, "src", "bootstrap", "main.ts"),

    module: {
        rules: [
            {
                test: /\.tsx?$/,
                exclude: /node_modules/,
                use: [
                    {
                        loader: 'babel-loader',
                        options: {
                            plugins: [
                                isDevelopment && require.resolve("react-refresh/babel")
                            ].filter(plugin => !!plugin)
                        }
                    }
                ]
            }
        ]
    },

    resolve: {
        extensions: [".tsx", ".ts", ".jsx", ".js"],
    },

    plugins: [
        new DotEnvPlugin({
            defaults: true,
            systemvars: true,
        }),
        new HtmlWebpackPlugin({
            title: "Simple Screen Share"
        }),
        isDevelopment && new ReactRefreshPlugin(),
    ].filter(plugin => !!plugin),

    output: {
        publicPath: "/"
    },

    devServer: {
        hot: false,
        liveReload: false,
        
        historyApiFallback: true
    }
} as Configuration;