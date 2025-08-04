class AddUrlToPayment < ActiveRecord::Migration[6.0]
  def up
    add_column :payments, :url, :string
    Payment.find_each do |payment|
      next if payment.url.present?

      case payment.type
      when 'Payments::Zen'
        payment.update!(url: "https://secure.zen.com/#{payment.payment_id}")
        payment.update!(payment_id: payment.id)
      when 'Payments::Bitcoin'
        payment.update!(url: "https://pay2.deltabadger.com/invoice?id=#{payment.payment_id}")
      end
    end
  end

  def down
    Payment.find_each do |payment|
      next if payment.url.blank?

      case payment.type
      when 'Payments::Zen'
        payment.update!(payment_id: payment.url.split('/').last.split('?').first)
      end
    end
    remove_column :payments, :url
  end
end
