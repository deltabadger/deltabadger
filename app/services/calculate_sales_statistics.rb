class CalculateSalesStatistics < BaseService
  def call
    @paid_payments = Payment.where(status: 'paid').includes(:user)
    @vat_rates = VatRate.pluck(:country, :vat).to_h

    total_sum = sum_of_paid_payments
    sum_of_first_month = sum_of_nth_month(0)
    sum_of_second_month = sum_of_nth_month(1)
    sum_of_third_month = sum_of_nth_month(2)
    rest = total_sum - (sum_of_first_month + sum_of_second_month + sum_of_third_month)

    total_sum = total_sum.nonzero? || 1
    {
      first: get_percentage(sum_of_first_month / total_sum),
      second: get_percentage(sum_of_second_month / total_sum),
      third: get_percentage(sum_of_third_month / total_sum),
      later: get_percentage(rest / total_sum)
    }
  end

  private

  def sum_of_paid_payments
    sum_payments(@paid_payments)
  end

  def sum_of_nth_month(n)
    payments = @paid_payments.select { |p| nth_month?(p, n) }
    sum_payments(payments)
  end

  def nth_month?(payment, n)
    return false unless payment.total.present?

    shifted_creation_date = payment.user.created_at + n.months

    shifted_creation_date.month == payment.paid_at.month &&
      shifted_creation_date.year == payment.paid_at.year
  end

  def sum_payments(payments)
    payments.reduce(0) do |accumulator, payment|
      accumulator + usd_netto_value(payment)
    end
  end

  def get_percentage(value)
    (value * 100).ceil(1)
  end

  def usd_netto_value(payment)
    return 0.0 unless payment.total.present?

    value = payment.total
    vat_rate = @vat_rates.fetch(payment.country, 0)
    value /= (1.0 + vat_rate)
    # constant EUR/USD rate
    value *= 1.2 if payment.currency.downcase == 'eur'

    value
  end
end
