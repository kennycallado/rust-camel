return {
  {
    "echasnovski/mini.nvim",
    version = false,
    config = function()
      local clues = {}
      local function map(mode, lhs, rhs, desc)
        vim.keymap.set(mode, lhs, rhs, { desc = desc })
        table.insert(clues, { mode = mode, keys = lhs, desc = desc })
      end

      require("mini.basics").setup({
        options = { basic = true, extra_ui = true, win_borders = "rounded" },
        mappings = { basic = true, option_toggle = true, windows = true, move_with_alt = true },
        autocommands = { relnum_in_visual_mode = true },
      })

      require("mini.ai").setup()
      require("mini.surround").setup()
      require("mini.comment").setup()
      require("mini.pairs").setup()
      require("mini.icons").setup()
      require("mini.bracketed").setup()
      require("mini.indentscope").setup({ symbol = "│" })
      require("mini.statusline").setup({ use_icons = true })
      require("mini.tabline").setup()

      require("mini.files").setup()
      map("n", "<leader>e", "<cmd>lua MiniFiles.open()<cr>", "File explorer")
      map("n", "<leader>fe", "<cmd>lua MiniFiles.open()<cr>", "File explorer")

      require("mini.pick").setup()
      require("mini.extra").setup()
      map("n", "<leader><space>", "<cmd>Pick files<cr>", "Find files")
      map("n", "<leader>ff", "<cmd>Pick files<cr>", "Find files")
      map("n", "<leader>fg", "<cmd>Pick grep_live<cr>", "Grep")
      map("n", "<leader>fb", "<cmd>Pick buffers<cr>", "Buffers")
      map("n", "<leader>,", "<cmd>Pick buffers<cr>", "Switch buffer")
      map("n", "<leader>:", "<cmd>Pick command_history<cr>", "Command history")

      require("mini.git").setup()
      map("n", "<leader>gs", "<cmd>Git status<cr>", "Git status")
      map("n", "<leader>gl", "<cmd>Git log --oneline<cr>", "Git log")
      map("n", "<leader>gf", "<cmd>Pick git_files<cr>", "Git files")
      map("n", "<leader>gc", "<cmd>Pick git_commits<cr>", "Git commits")
      map("n", "<leader>gB", "<cmd>Pick git_branches<cr>", "Git branches")

      require("mini.clue").setup({
        window = { config = { anchor = "SW" } },
        triggers = {
          { mode = "n", keys = "<leader>" },
          { mode = "x", keys = "<leader>" },
          { mode = "n", keys = "g" },
          { mode = "x", keys = "g" },
          { mode = "n", keys = "'" },
          { mode = "n", keys = "`" },
          { mode = "n", keys = '"' },
          { mode = "n", keys = "<C-w>" },
          { mode = "n", keys = "z" },
        },
        clues = {
          require("mini.clue").gen_clues.builtin_completion(),
          require("mini.clue").gen_clues.g("g"),
          require("mini.clue").gen_clues.marks(),
          require("mini.clue").gen_clues.registers(),
          require("mini.clue").gen_clues.windows(),
          require("mini.clue").gen_clues.z(),
          { mode = "n", keys = "<leader>f", desc = "+file/find" },
          { mode = "n", keys = "<leader>g", desc = "+git" },
          { mode = "n", keys = "<leader>gh", desc = "+git/hunk" },
          { mode = "n", keys = "<leader>t", desc = "+terminal" },
          { mode = "x", keys = "<leader>f", desc = "+file/find" },
        },
      })
    end,
  },
}
