# Spreadsheet software treats a cell beginning with = + - @ (or a leading control
# character) as a formula. Exchange-supplied text reaches our exports, so prefix a
# quote to force it to be read as text. Non-strings are left alone so numeric
# columns stay numeric.
module CsvSafe
  FORMULA_LEADERS = ['=', '+', '-', '@', "\t", "\r", "\n"].freeze

  def self.cell(value)
    return value unless value.is_a?(String)
    return value if value.empty?

    FORMULA_LEADERS.any? { |leader| value.start_with?(leader) } ? "'#{value}" : value
  end

  def self.row(values)
    Array(values).map { |value| cell(value) }
  end

  # Drop-in replacement for CSV.generate. Escaping at the WRITER rather than at each
  # call site matters here: app/models/tax/report.rb alone has ~40 `csv <<` sites, most
  # of them I18n literals but several carrying exchange-derived asset names and price
  # warnings. Wrapping each one individually would be forgotten the first time someone
  # adds a row.
  def self.generate(&block)
    CSV.generate do |csv|
      block.call(Writer.new(csv))
    end
  end

  class Writer
    def initialize(csv)
      @csv = csv
    end

    def <<(row)
      @csv << CsvSafe.row(row)
      self
    end

    def method_missing(name, *args, &blk) = @csv.send(name, *args, &blk)
    def respond_to_missing?(name, include_private = false) = @csv.respond_to?(name, include_private)
  end
end
