# Test-only counterfactual oracle for R4. First record Rails unchanged; only the second reading
# supplies normalized decimal rows to Rails' existing accounting. No database/app source writes.
module NormalizedFigureRows
  def pluck(*columns)
    return super unless Thread.current[:normalized_figure_rows] && klass == Transaction && columns.include?(:amount_exec) && columns.include?(:quote_amount_exec) && columns.include?(:price)

    # The original chart query excludes NULL-price rows before pluck; known value makes those usable.
    Thread.current[:normalized_figure_rows] = false
    rows = unscope(where: :price).pluck(*columns)
    Thread.current[:normalized_figure_rows] = true
    rows.map do |row|
      values = columns.zip(row).to_h
      quantity = values[:amount_exec] || (values[:external_status] == 'closed' ? values[:amount] : nil)
      next row unless quantity.to_d.positive?

      value = values[:quote_amount_exec].to_d
      value = values[:price].to_d * quantity if !value.positive? && values[:price].to_d.positive?
      next row unless value.positive? # unavailable cases are asserted separately, never given fake figures

      normalized = { amount_exec: quantity, quote_amount_exec: value, price: value / quantity }
      columns.each_with_index.map { |column, i| normalized.fetch(column, row[i]) }
    end
  end
end
ActiveRecord::Relation.prepend(NormalizedFigureRows)
