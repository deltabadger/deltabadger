desc 'rake task to update transactions quote amount'
task update_transactions_data: :environment do
  update_transactions_side
  update_transactions_remote_data
end

def update_transactions_side
  puts 'updating transactions side'
  buy_bot_ids = Bot.basic.where("settings @> ?", {type: "buy"}.to_json).pluck(:id)
                   .concat(Bot.not_legacy.pluck(:id))
  sell_bot_ids = Bot.basic.where("settings @> ?", {type: "sell"}.to_json).pluck(:id)
  Transaction.where(bot_id: buy_bot_ids).where(side: nil).update_all(side: :buy)
  Transaction.where(bot_id: sell_bot_ids).where(side: nil).update_all(side: :sell)
end

def update_transactions_remote_data
  puts 'updating transactions remote data'
  exchanges = Exchange.where(name_id: ["coinbase", "kraken"])
  exchange_ids = exchanges.pluck(:id)
  puts 'getting bot ids'
  bot_ids = Transaction.submitted.where(exchange_id: exchange_ids, external_status: nil).where.not(external_id: nil).pluck(:bot_id).uniq
  puts 'getting user ids'
  user_ids = Bot.where(id: bot_ids).pluck(:user_id).uniq
  user_ids.sort.reverse.each do |user_id|
    user = User.find(user_id)
    exchanges.each do |exchange|
      api_key = user.api_keys.trading.find_by(exchange: exchange)
      next if api_key.blank? || api_key.incorrect?

      puts "checking valid api key for #{exchange.name} for user #{user.id}"
      result = exchange.check_valid_api_key?(api_key: api_key)
      if result.failure?
        puts "failed to check valid api key for #{exchange.name} for user #{user.id}: #{result.errors.to_sentence}"
        next
      end

      valid = result.data
      if !valid
        puts "invalid api key for #{exchange.name} for user #{user.id}"
        api_key.update!(status: :incorrect)
        next
      end

      puts "setting client for #{exchange.name} for user #{user.id}"
      exchange.set_client(api_key: api_key)

      user_bots_ids = user.bots.basic.where(exchange: exchange).pluck(:id).uniq
      puts "getting transactions for #{exchange.name} for user #{user.id} for bots #{user_bots_ids}"
      Transaction.submitted.where(bot_id: user_bots_ids, exchange: exchange, external_status: nil)
                 .where.not(external_id: nil).find_each do |transaction|
        puts "getting order for #{transaction.external_id}"
        begin
          result = exchange.get_order(order_id: transaction.external_id)
          # sleep 0.5
        rescue KeyError => e
          puts "error getting order for #{transaction.external_id} (#{transaction.created_at}): #{e.message}"
          # sleep 0.5
          next
        end
        if result.failure?
          puts "error getting order for #{transaction.external_id} (#{transaction.created_at}): #{result.errors.to_sentence}"
          break
        end

        quote_amount = result.data[:quote_amount] || (result.data[:price] * result.data[:amount]).to_d
        side = result.data[:side]
        filled_percentage = result.data[:filled_percentage]
        external_status = result.data[:status]
        puts "updating transaction #{transaction.id} with quote amount #{quote_amount}, side #{side}, filled percentage #{filled_percentage}, external status #{external_status}"

        transaction.update!(quote_amount: quote_amount, side: side, filled_percentage: filled_percentage, external_status: external_status)
      end
    end
  end
end
