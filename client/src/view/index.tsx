import { createTheme, CssBaseline, darkScrollbar, ThemeProvider } from "@mui/material";
import React from "react";
import { QueryCache, QueryClient, QueryClientProvider } from "react-query";
import { ReactQueryDevtools } from 'react-query/devtools';
import { Provider as StateProvider } from "react-redux";
import { appStore } from "../state";
import { ToastContainer } from "react-toastify";
import { BrowserRouter } from "react-router-dom";
import { Helmet } from "react-helmet";
import { AuthentificationGuard } from "./components/authentification";
import { Box } from "@mui/system";
import AppPages from "./pages";

export const queryClient = new QueryClient({
    queryCache: new QueryCache(),
    defaultOptions: {
        queries: {
            refetchOnWindowFocus: false,
            refetchOnReconnect: false,
            refetchOnMount: false,
            retry: false,
        },
    },
});

const darkTheme = createTheme({
    palette: {
        mode: 'dark',
    },
    components: {
        MuiCssBaseline: {
            styleOverrides: {
                html: {
                    ...darkScrollbar({
                        track: "#27272700",
                        thumb: "#585859",
                        active: "#838384"
                    }),
                    //scrollbarWidth for Firefox
                    scrollbarWidth: "thin",
                },
                ".MuiButton-root.Mui-disabled": {
                    pointerEvents: "auto!important",
                    cursor: "pointer!important",

                    "&:hover": {
                        background: "unset!important"
                    }
                }
            }
        }
    },
    typography: {
        fontSize: 13,
    },
    spacing: 7
});

export default React.memo(() => {
    return (
        <QueryClientProvider client={queryClient}>
            <ReactQueryDevtools initialIsOpen={false} />
            <StateProvider store={appStore}>
                <ThemeProvider theme={darkTheme}>
                    <Helmet>
                        <title>Simple Screen Share</title>
                    </Helmet>
                    <CssBaseline enableColorScheme={true}/>
                    <Box sx={{
                        width: "100vw",
                        height: "100vh",

                        display: "flex",
                        flexDirection: "column",
                    }}>
                        <BrowserRouter>
                            <AuthentificationGuard>
                                <AppPages />
                            </AuthentificationGuard>
                        </BrowserRouter>
                    </Box>
                </ThemeProvider>
                <ToastContainer />
            </StateProvider>
        </QueryClientProvider>
    );
});