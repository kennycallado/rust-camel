local terminals = {}

local function toggle_terminal(i)
  local Terminal = require("toggleterm.terminal").Terminal
  terminals[i] = terminals[i] or Terminal:new({
    id = i,
    display_name = "term-" .. i,
    direction = "float",
  })
  terminals[i]:toggle()
end

return {
  {
    "akinsho/toggleterm.nvim",
    version = "*",
    opts = {
      open_mapping = [[<C-\>]],
      direction = "float",
      shade_terminals = true,
      float_opts = {
        border = "curved",
        width = function()
          return math.floor(vim.o.columns * 0.8)
        end,
        height = function()
          return math.floor(vim.o.lines * 0.8)
        end,
      },
    },
    config = function(_, opts)
      require("toggleterm").setup(opts)
      vim.keymap.set("t", "<S-Esc>", [[<C-\><C-n>]], { desc = "Exit terminal mode" })
      vim.keymap.set({ "n", "t" }, "<leader>tt", "<cmd>ToggleTerm<CR>", { desc = "Toggle terminal" })
      for i = 1, 9 do
        vim.keymap.set({ "n", "t" }, "<C-" .. i .. ">", function()
          toggle_terminal(i)
        end, { desc = "Toggle terminal " .. i })
        vim.keymap.set("n", "<leader>t" .. i, function()
          toggle_terminal(i)
        end, { desc = "Toggle terminal " .. i })
      end
    end,
  },
}
