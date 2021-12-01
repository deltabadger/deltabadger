require 'csv'

module Admin
  class GeneratePaymentsCsv < BaseService
    class GenerateCsv < BaseService
      def call(data)
        return '' if data.empty?

        CSV.generate do |csv|
          csv << data.first.keys
          data.each do |row|
            csv << row.values
          end
        end
      end
    end

    def initialize(
      payments_repository: PaymentsRepository.new,
      generate_csv: GenerateCsv.new
    )

      @payments_repository = payments_repository
      @generate_csv = generate_csv
    end

    def call(from:, to:, wire:)
      payments = @payments_repository.paid_between(from: from, to: to, wire: wire)
      formatted_data = payments.map { |p| format_payment(p, wire) }
      @generate_csv.call(formatted_data)
    end

    private

    def format_payment(payment, wire)
      format = {
        id: payment.id,
        total: payment.total,
        currency: payment.currency,
        first_name: payment.first_name,
        last_name: payment.last_name,
        birth_date: payment.birth_date&.strftime('%F'),
        country: payment.country,
        paid_at: payment.paid_at,
        user: payment.user.email,
        payment_id: payment.payment_id
      }
      format.merge(crypto_paid: payment.crypto_paid) unless wire
      format
    end
  end
end
