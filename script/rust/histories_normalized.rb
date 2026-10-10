# Test-only extension: Accountable omits the quantity needed by the shared figures oracle.
# Preserve requested price/amount for waiting commitments; normalize only settled credits.
# Two reads are safe here: these synthetic fixtures are inside a private transaction, with
# all venue access disabled. Production retains a single SELECT in fill::commitments.
require Rails.root.join('script/rust/figures_normalized')
module NormalizedHistoryCredits
  def pluck(*columns)
    return super unless Thread.current[:normalized_figure_rows] && klass == Transaction
    credit = columns == %i[external_status quote_amount amount price quote_amount_exec]
    closed_cap = columns == [:quote_amount_exec]
    stopped_cap = columns.length == 1 && columns.first.to_s == 'COALESCE(quote_amount_exec, 0)'
    return super unless credit || closed_cap || stopped_cap
    requested = credit ? columns + [:amount_exec] : columns + %i[external_status amount price amount_exec quote_amount_exec]

    begin
      Thread.current[:normalized_figure_rows] = false
      original = super(*requested)
    ensure
      Thread.current[:normalized_figure_rows] = true
    end
    normalized = super(*requested)
    raise 'History recorder snapshot changed' unless original.length == normalized.length

    original.zip(normalized).map do |raw, accepted|
      if credit
        raw[4] = accepted[4] unless %w[open unknown].include?(raw[0])
        raw.first(columns.length)
      elsif stopped_cap && raw[0].to_d.positive?
        raw[0] # preserve SQL numeric kind for Rails' existing stopped-bucket sum
      else
        accepted[-1] || raw[0]
      end
    end
  end
end
ActiveRecord::Relation.prepend(NormalizedHistoryCredits)
