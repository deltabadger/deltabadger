# Spreadsheet software treats a cell beginning with = + - @ (or a leading control
# character) as a formula, including when that character follows leading whitespace.
# Exchange-supplied text reaches our exports, so prefix a quote to force it to be read
# as text. Non-strings are left alone so numeric columns stay numeric.
module CsvSafe
  FORMULA_LEADERS = ['=', '+', '-', '@', "\t", "\r", "\n"].freeze

  def self.cell(value)
    return value unless value.is_a?(String)
    return value if value.empty?

    leading_char?(value) ? "'#{value}" : value
  end

  def self.row(values)
    Array(values).map { |value| cell(value) }
  end

  # Inverse of cell: strips the single leading quote it adds, so a value round-tripped
  # through an export and back (e.g. Bot::Exportable#import_orders_csv) matches the
  # original. cell only ever adds that quote ahead of a formula leader, so only strip it
  # here when the remainder is itself formula-leading -- otherwise a value that legitimately
  # starts with an apostrophe (never touched by cell, since "'" isn't a formula leader) would
  # be corrupted on the way back in. This isn't a perfect inverse: cell("=1+1") and the literal
  # string "'=1+1" both export as "'=1+1", so a literal apostrophe followed by a formula leader
  # ('-5, '@G, '+1, '\tx, ' =1+1, ...) stays ambiguous. Every other case -- an apostrophe
  # followed by a non-formula character, which is the overwhelmingly common one -- now
  # round-trips correctly, where before this stripped every leading apostrophe unconditionally.
  def self.unescape(value)
    return value unless value.is_a?(String)
    return value unless value.start_with?("'")

    remainder = value.delete_prefix("'")
    leading_char?(remainder) ? remainder : value
  end

  # Minimal wrapper around CSV.generate: takes only a block, escaping every row written
  # through it. It does not forward writer options (col_sep, force_quotes, etc.) — none
  # of the three current call sites need them. Escaping at the WRITER rather than at
  # each call site matters here: app/models/tax/report.rb alone has ~40 `csv <<` sites,
  # most of them I18n literals but several carrying exchange-derived asset names and
  # price warnings. Wrapping each one individually would be forgotten the first time
  # someone adds a row.
  def self.generate(&block)
    CSV.generate do |csv|
      block.call(Writer.new(csv))
    end
  end

  def self.leading_char?(value)
    FORMULA_LEADERS.any? { |leader| value.start_with?(leader) || value.lstrip.start_with?(leader) }
  end
  private_class_method :leading_char?

  class Writer
    def initialize(csv)
      @csv = csv
    end

    def <<(row)
      @csv << CsvSafe.row(row)
      self
    end

    # CSV#add_row and CSV#puts are aliases of CSV#<<. Alias them here too instead of
    # relying on delegation, so every way of writing a row goes through the escape —
    # a delegated method would write straight to the underlying CSV unescaped.
    alias add_row <<
    alias puts <<
  end
end
