module Utilities
  module String
    def self.numeric?(string)
      string.match?(/\A-?\d+(\.\d+)?([eE]-?\d+)?\z/)
    end
  end
end
