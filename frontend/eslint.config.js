// Flat config（eslint 9）。enforcement 规则见方案 §9.5.1 / §9.5.6。
import tseslint from "typescript-eslint";
import lit from "eslint-plugin-lit";
import boundaries from "eslint-plugin-boundaries";

export default tseslint.config(
  { ignores: ["dist/**", "node_modules/**"] },
  ...tseslint.configs.recommended,
  lit.configs["flat/recommended"],
  {
    plugins: {
      boundaries
    },
    settings: {
      "boundaries/include": ["src/**"]
    },
    rules: {
      // 壳检测收敛：全仓只有 bridge/tauri.ts 可触碰 __TAURI__
      "no-restricted-globals": ["error", "__TAURI__"],

      // 协议调用收敛：bridge 协议文件仅 stores 可 import
      "no-restricted-imports": [
        "error",
        {
          patterns: [
            {
              group: ["**/bridge/call", "**/bridge/stream", "**/bridge/session-event"],
              message:
                "协议 bridge 只允许 platform/stores 调用；组件请走 store action（方案 §9.5.1）。"
            }
          ]
        }
      ]
    }
  },
  {
    // tauri.ts 独享豁免
    files: ["src/platform/bridge/tauri.ts"],
    rules: {
      "no-restricted-globals": "off"
    }
  },
  {
    // bridge 协议文件本身允许相互 import
    files: [
      "src/platform/bridge/call.ts",
      "src/platform/bridge/stream.ts",
      "src/platform/bridge/session-event.ts"
    ],
    rules: {
      "no-restricted-imports": "off"
    }
  },
  {
    // stores 是协议 bridge 的唯一合法消费方（方案 §9.5.1 白名单）
    files: ["src/platform/stores/**/*.ts"],
    rules: {
      "no-restricted-imports": "off"
    }
  },
  {
    files: ["**/*.ts"],
    rules: {
      "@typescript-eslint/no-unused-vars": [
        "error",
        { argsIgnorePattern: "^_", varsIgnorePattern: "^_" }
      ]
    }
  }
);
