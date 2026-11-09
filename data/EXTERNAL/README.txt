EXTERNAL — programs a connected station can run

Remote must be on. With Remote off, //RUN is answered with
"<GPNG>: Remote commands are turned off by the sysop."

//RUN
    starts nothing. It sends data/RUN.GPI.

//RUN ?
    lists the programs in this folder, and says which languages
    were found on this computer

//RUN FREE
    memory total, used, and still free. It reads no input.

//RUN DOS
    an MS-DOS 6.20 window. Drive C: is the USER folder, and there
    is no other drive (D: answers "Drive D does not excist!").
    dir, cd, echo, type, help, and exit, in any letter case.
    DIR switches may touch the command or the name: dir/w,
    dir/ad, and dir *.txt/ad all work. exit closes the program.

//FREE
    the same program. //DOS does the same for DOS. A name that is
    not already a GP command can be started without the word RUN.

Only this folder can be started. A slash, a drive letter, or ..
is refused. The other station cannot point the command at the
rest of the disk, and it cannot upload the program.

Linux, macOS, and Windows
    The first line may name the program, the way Linux does:

        #! /usr/bin/env python3
        #! /usr/bin/perl
        #! /usr/lib/cgi-bin/php7.2
        #! /usr/bin/env java --source 11

    GPNG reads that line on every system and starts the program
    if it is installed. The ones it will start are Python, PHP,
    Perl, Ruby, Node, Lua, and Java. Each of those runs in the
    background and only its printed text comes back. Java is
    started headless, so it cannot open a window. A shell, a
    batch file, or a Windows program is refused.
    macOS only honours that first line for files marked
    executable, and Windows does not do it at all, so the program
    is started from here instead of by the operating system.
    A Linux path that is not on this computer falls back to the
    same program on PATH. A .py, .php, .pl, .rb, .js, .lua,
    .java, or .jar name is enough when the file has no first
    line of that kind.

    The script's printed text is sent back to the station that
    started it. If the script reads a line, the next text from
    that connection is the line, and that same program stays
    open while it waits. A packet for a running program is one
    line for it at once; it is not held until the next packet
    brings an Enter. A second //RUN on that channel answers
    "program is running" and does not start another one.
    When the script exits, the process is closed. A game that
    is still printing, or still being typed to, stays open.
    If 300 seconds pass with no typed line and no new text,
    it is closed. A station that is still connected is told
    "closed run program after timeout". Disconnect closes it
    as well. One program per channel, so ten channels is ten
    programs at most.

FREE and DOS are the samples. Both are Python and use only the
standard library. FREE reports memory on Linux, macOS, and
Windows and then exits. DOS stays open until EXIT, the link
drops, or the 300-second quiet limit closes it.
