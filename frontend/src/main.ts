import { mount } from "svelte";
import App from "./App.svelte";
import "./design.css";
import "./platform-typography.css";

mount(App, { target: document.getElementById("app")! });
