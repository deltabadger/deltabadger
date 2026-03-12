# frozen_string_literal: true

class ListBotsTool < ApplicationMCPTool
  tool_name 'list_bots'
  description 'List all DCA bots with their status, type, trading pair, and exchange'
  read_only

  property :status, type: 'string', description: 'Filter by status: scheduled, executing, waiting, retrying, stopped, created (optional)'

  def perform
    user = current_user
    bots = user.bots.not_deleted.includes(:exchange)

    bots = bots.where(status: status) if status.present?

    if bots.empty?
      render text: 'No bots found.'
      return
    end

    lines = bots.map do |bot|
      base = bot.respond_to?(:base_asset) ? bot.base_asset&.symbol : nil
      quote = bot.quote_asset&.symbol

      pair = if bot.dca_dual_asset?
               "#{bot.base0_asset&.symbol}+#{bot.base1_asset&.symbol}/#{quote}"
             elsif base && quote
               "#{base}/#{quote}"
             else
               'N/A'
             end

      interval = bot.settings['interval'] || 'N/A'
      amount = bot.settings['quote_amount']

      "- #{bot.label} | #{bot.type.demodulize.titleize} | #{pair} | #{bot.exchange&.name || 'N/A'} | #{bot.status} | #{amount} #{quote}/#{interval}"
    end

    render text: "Bots (#{bots.size}):\n#{lines.join("\n")}"
  end
end
