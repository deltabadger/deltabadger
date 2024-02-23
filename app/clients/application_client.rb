class ApplicationClient
  OPTIONS = {
    request: {
      open_timeout: 5,      # seconds to wait for the connection to open
      read_timeout: 5,      # seconds to wait for one block to be read
      write_timeout: 5      # seconds to wait for one block to be written
    }
  }.freeze
end
