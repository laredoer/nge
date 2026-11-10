// 平台跳跃的入口：游戏本身在 game.tsx（关卡编辑器试玩时也用它）。
import { mount } from "nsh";
import { Game } from "./game";

mount(() => <Game />);
