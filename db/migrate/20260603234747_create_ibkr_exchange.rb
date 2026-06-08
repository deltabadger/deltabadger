class CreateIbkrExchange < ActiveRecord::Migration[8.1]
  def up
    Exchanges::Ibkr.find_or_create_by!(name: 'Interactive Brokers') do |exchange|
      exchange.maker_fee = '0.0'
      exchange.taker_fee = '0.0'
      exchange.available = true
    end
  end

  def down
    Exchanges::Ibkr.find_by(name: 'Interactive Brokers')&.destroy
  end
end
