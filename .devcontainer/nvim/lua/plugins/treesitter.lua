return {
  {
    "nvim-treesitter/nvim-treesitter",
    branch = "master",
    build = ":TSUpdate",
    config = function()
      require("nvim-treesitter.configs").setup({
        ensure_installed = {
          "rust",
          "toml",
          "lua",
          "vim",
          "vimdoc",
          "query",
          "bash",
          "markdown",
          "markdown_inline",
          "json",
          "yaml",
        },
        sync_install = false,
        auto_install = true,
        highlight = { enable = true, disable = { "markdown", "markdown_inline" } },
        indent = { enable = true, disable = { "markdown", "markdown_inline" } },
      })
      vim.api.nvim_create_autocmd("FileType", {
        pattern = { "markdown", "markdown.mdx" },
        callback = function(args)
          pcall(vim.treesitter.stop, args.buf)
        end,
      })
    end,
  },
}
