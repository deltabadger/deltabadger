# frozen_string_literal: true

module BotApi
  module Rules
    class List
      def self.call(user:)
        rows = user.rules.where(type: 'Rules::Withdrawal').where.not(status: :deleted)
                   .includes(:exchange, :asset).order(:id).map { |rule| row_for(rule) }
        Result.success({ count: rows.size, rules: rows })
      end

      # Enough of the destination to recognise it, never enough to send to it — at any length.
      # Real addresses are 26+ characters; a short one is shown as a prefix only.
      def self.mask(address)
        text = address.to_s
        return '…' if text.length <= 4
        return "#{text[0, 2]}…" if text.length <= 12

        "#{text[0, 6]}…#{text[-4, 4]}"
      end

      def self.row_for(rule)
        {
          id: rule.id, type: rule.type, status: rule.status.to_s,
          exchange: rule.exchange&.name, asset: rule.asset&.symbol,
          address: mask(rule.address), address_tag: rule.address_tag, network: rule.network,
          threshold_type: rule.threshold_type, withdrawal_percentage: rule.withdrawal_percentage,
          max_fee_percentage: rule.max_fee_percentage, min_amount: rule.min_amount
        }
      end
    end
  end
end
