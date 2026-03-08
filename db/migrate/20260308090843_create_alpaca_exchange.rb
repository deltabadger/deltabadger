class CreateAlpacaExchange < ActiveRecord::Migration[8.1]
  def up
    Exchanges::Alpaca.find_or_create_by!(name: "Alpaca") do |exchange|
      exchange.maker_fee = "0.0"
      exchange.taker_fee = "0.0"
      exchange.available = true
    end
  end

  def down
    Exchanges::Alpaca.find_by(name: "Alpaca")&.destroy
  end
end
