# frozen_string_literal: true

class ListBotsTool < ApplicationMCPTool
  tool_name 'list_bots'
  description 'List all DCA bots with their status, type, trading pair, and exchange'
  read_only

  property :status, type: 'string', description: 'Filter by status: scheduled, executing, waiting, retrying, stopped, created (optional)'

  def perform
    result = BotApi::Bots::List.call(user: current_user, status: status)

    if result.data[:count].zero?
      render text: 'No bots found.'
      return
    end

    render text: present(result.data)
  end

  private

  def present(data)
    lines = data[:bots].map do |row|
      pair = row[:pair] || 'N/A'
      exchange = row[:exchange] || 'N/A'
      interval = row[:interval] || 'N/A'
      type_label = row[:type].to_s.demodulize.titleize

      "- #{row[:label]} | #{type_label} | #{pair} | #{exchange} | #{row[:status]} | #{row[:quote_amount]} #{row[:quote_asset]}/#{interval}"
    end

    "Bots (#{data[:count]}):\n#{lines.join("\n")}"
  end
end
