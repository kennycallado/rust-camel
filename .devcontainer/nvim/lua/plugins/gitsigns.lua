return {
  {
    "lewis6991/gitsigns.nvim",
    event = { "BufReadPre", "BufNewFile" },
    config = function()
      require("gitsigns").setup({
        signs = {
          add = { text = "+" },
          change = { text = "~" },
          delete = { text = "_" },
          topdelete = { text = "‾" },
          changedelete = { text = "~" },
        },
      })
      local function map(mode, lhs, rhs, desc)
        vim.keymap.set(mode, lhs, rhs, { desc = desc })
      end
      map("n", "]h", "<cmd>Gitsigns next_hunk<cr>", "Next hunk")
      map("n", "[h", "<cmd>Gitsigns prev_hunk<cr>", "Prev hunk")
      map({ "n", "v" }, "<leader>ghs", ":Gitsigns stage_hunk<cr>", "Stage hunk")
      map({ "n", "v" }, "<leader>ghr", ":Gitsigns reset_hunk<cr>", "Reset hunk")
      map("n", "<leader>ghS", "<cmd>Gitsigns stage_buffer<cr>", "Stage buffer")
      map("n", "<leader>ghu", "<cmd>Gitsigns undo_stage_hunk<cr>", "Undo stage hunk")
      map("n", "<leader>ghR", "<cmd>Gitsigns reset_buffer<cr>", "Reset buffer")
      map("n", "<leader>ghp", "<cmd>Gitsigns preview_hunk<cr>", "Preview hunk")
      map("n", "<leader>ghb", "<cmd>Gitsigns blame_line<cr>", "Blame line")
      map("n", "<leader>ghd", "<cmd>Gitsigns diffthis<cr>", "Diff this")
      map("n", "<leader>gb", "<cmd>Gitsigns toggle_current_line_blame<cr>", "Toggle line blame")
      map({ "o", "x" }, "ih", ":<C-U>Gitsigns select_hunk<cr>", "Git hunk")
    end,
  },
}
