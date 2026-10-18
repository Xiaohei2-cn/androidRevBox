/** @type {import('tailwindcss').Config} */
export default {
  darkMode: ["class"],
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  theme: {
    extend: {
      /**
       * 排版基线（P7 用户反馈：字体统一调大 2 号）。
       * 整体上移一档而非逐文件改类名——避免遗漏、也保证后续新增页面风格一致：
       *   xs 12→14（原正文主力）、sm 14→16、base 16→18、
       *   10px→12px、11px→13px（原微标签与齿块标签）。
       * 组件高度/padding 固定，字号变大后靠既有 flex 居中兜住。
       */
      fontSize: {
        xs: ["0.875rem", { lineHeight: "1.25rem" }],
        sm: ["1rem", { lineHeight: "1.5rem" }],
        base: ["1.125rem", { lineHeight: "1.75rem" }],
        lg: ["1.25rem", { lineHeight: "1.75rem" }],
        "10px": ["0.75rem", { lineHeight: "1rem" }],
        "11px": ["0.8125rem", { lineHeight: "1rem" }],
      },
      colors: {
        background: "hsl(var(--background))",
        foreground: "hsl(var(--foreground))",
        card: {
          DEFAULT: "hsl(var(--card))",
          foreground: "hsl(var(--card-foreground))",
        },
        popover: {
          DEFAULT: "hsl(var(--popover))",
          foreground: "hsl(var(--popover-foreground))",
        },
        primary: {
          DEFAULT: "hsl(var(--primary))",
          foreground: "hsl(var(--primary-foreground))",
        },
        secondary: {
          DEFAULT: "hsl(var(--secondary))",
          foreground: "hsl(var(--secondary-foreground))",
        },
        muted: {
          DEFAULT: "hsl(var(--muted))",
          foreground: "hsl(var(--muted-foreground))",
        },
        accent: {
          DEFAULT: "hsl(var(--accent))",
          foreground: "hsl(var(--accent-foreground))",
        },
        destructive: {
          DEFAULT: "hsl(var(--destructive))",
          foreground: "hsl(var(--destructive-foreground))",
        },
        border: "hsl(var(--border))",
        input: "hsl(var(--input))",
        ring: "hsl(var(--ring))",
      },
      borderRadius: {
        lg: "var(--radius)",
        md: "calc(var(--radius) - 2px)",
        sm: "calc(var(--radius) - 4px)",
      },
      boxShadow: {
        /**
         * 卡片浮起（tauri-ui 的桌面刻度：分层叠加，不是一道大阴影）：
         * 1px 贴边负责"贴住页面"，16px 那一层负责"浮起来"。
         * 透明窗口下 alpha 必须压得很低，否则叠在渐变上就是一圈脏边。
         */
        card: "0 1px 2px rgb(17 24 39 / 0.04), 0 4px 16px rgb(17 24 39 / 0.05)",
        "card-hover": "0 2px 4px rgb(17 24 39 / 0.06), 0 10px 28px rgb(17 24 39 / 0.09)",
        /** 可交互面（按钮/输入）的轻浮起；与卡片同一 alpha 基准，叠在一起不会脏 */
        control: "0 1px 2px rgb(17 24 39 / 0.05)",
        /**
         * 属性胶囊的底板：**内阴影**，不是外阴影。
         * 权限/端口这类属性读作"刻进表格里的一小块凹槽"，外阴影会让它看起来像浮着的按钮，
         * 与行内真正的动作按钮混淆。inset 顶部压暗 + 一圈极淡描边 = 浅浅的内凹。
         */
        pill: "inset 0 1px 2px rgb(17 24 39 / 0.07), inset 0 0 0 1px rgb(17 24 39 / 0.04)",
        /**
         * 列表气泡行：极轻的一层外阴影把它从卡片底上"托"起来半毫米。
         * 只有 1px 与 alpha 0.03——再多就会在透明窗口的渐变上显出一圈脏边，
         * 也会与卡片自身的 shadow-card 叠成两层灰边。
         */
        row: "0 1px 2px rgb(17 24 39 / 0.035)",
      },
      /**
       * 表格列宽的唯一定义处：设备页文件列表与其它"类表格"行共用同一组刻度。
       * 用固定 px 而不是 fr：文件大小、权限这类列必须**逐行对齐**，
       * fr 会随最长文件名浮动，读起来像每行各自排版。
       */
      spacing: {
        "col-perms": "8.75rem",
        "col-size": "5rem",
        "col-actions": "6.5rem",
      },
    },
  },
  plugins: [],
};
